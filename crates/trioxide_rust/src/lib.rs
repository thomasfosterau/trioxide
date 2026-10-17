//! The Rust backend: turns eligible functions into Rust source for a WASM island (PLAN §6).
//!
//! - [`eligibility`] decides whether a function is in the compilable subset (PLAN §5) and
//!   explains why not, with spans.
//! - [`rir`] is the small typed tree IR the backend lowers to from the oxc AST plus analysis
//!   [`Facts`]. Lowering doubles as the subset checker: a function is eligible exactly when
//!   it lowers.
//! - [`emit`] prints RIR as an island crate in the shape `docs/ABI.md` prescribes (via
//!   `prettyplease`), together with its [`IslandSpec`].
//! - [`forced_islands`] runs all of it for the functions source directives force into WASM
//!   (`"use wasm"`, `/** @trioxide wasm */`).
//!
//! The RIR stays backend-agnostic so a direct `wasm-encoder` backend remains possible (D6).

pub mod eligibility;
pub mod emit;
mod kinds;
mod lower;
mod names;
pub mod rir;
mod world;

use std::path::Path;

use oxc_allocator::Allocator;
use rustc_hash::FxHashSet;
use trioxide_analysis::{Fact, Facts};
use trioxide_common::{
    Diagnostic, ModuleId, Span, SymbolRef,
    log::{FunctionDecision, Target},
};
use trioxide_types::{FunctionId, Type};
use trioxide_wasm::IslandSource;
pub use trioxide_wasm::IslandSpec;

use crate::world::{Role, World};
pub use crate::{
    lower::Reason,
    world::{Directive, Placement, SourceModule},
};

/// Queries the backend would like from the analysis but that `Facts` doesn't answer yet. The
/// defaults are conservative ("nothing known"); the analysis can implement them as it grows.
pub trait ExtraFacts {
    /// The type of property `field` of every object that may reach parameter `param` when
    /// its function is entered, if known. Without it, the backend relies on the type facts of
    /// the island's reads of the field.
    fn param_field(&self, param: SymbolRef, field: &str) -> Option<Fact<Type>> {
        let _ = (param, field);
        None
    }
}

/// [`ExtraFacts`] with nothing beyond [`Facts`].
#[derive(Debug, Clone, Copy, Default)]
pub struct NoExtraFacts;

impl ExtraFacts for NoExtraFacts {}

/// An island generated from source.
#[derive(Debug, Clone)]
pub struct GeneratedIsland {
    /// The crate: spec plus `src/lib.rs`. The spec has no `fallback` yet; the caller, which
    /// knows where the JS module ends up, sets it when [`needs_fallback`](Self::needs_fallback).
    pub source: IslandSource,
    /// The module whose functions it exports.
    pub module: ModuleId,
    /// The exported functions with their JS names, in spec order.
    pub exports: Vec<(FunctionId, String)>,
    /// How JS code may reach the exports: only exports whose binding is never reassigned
    /// are listed, so calls to the others always stay JS.
    pub bindings: Vec<ExportBinding>,
    /// Every function compiled into it (exports and the functions they call).
    pub members: Vec<FunctionId>,
    /// Some export takes two arrays or structs and writes to one, so the glue needs the JS
    /// versions for calls that pass overlapping ones (`docs/ABI.md`, "Aliasing").
    pub needs_fallback: bool,
    /// The lowered island.
    pub rir: rir::Island,
}

/// An island export whose binding its module never reassigns, so a direct call of it can call
/// the island instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportBinding {
    /// The island export's name (also the function's name in its module).
    pub name: String,
    /// The names its module exports it under (`default` included).
    pub exported_as: Vec<String>,
}

/// The result of compiling the directive-forced islands of a program.
#[derive(Debug, Default)]
pub struct ForcedIslands {
    /// Islands to build, one per module with forced functions.
    pub islands: Vec<GeneratedIsland>,
    /// Per-function verdicts for the decision log.
    pub decisions: Vec<(ModuleId, FunctionDecision)>,
    /// Errors for forced functions that can't be compiled, and directives in the wrong
    /// place.
    pub diagnostics: Vec<Diagnostic>,
}

/// Whether `source` may contain a directive, so programs without any skip parsing.
fn mentions_directive(source: &str) -> bool {
    source.contains("use wasm") || source.contains("use js") || source.contains("@trioxide")
}

/// Compiles every function that a `"use wasm"` directive (file or function) or a
/// `/** @trioxide wasm */` tag forces into WASM: one island per module, holding its forced
/// functions as exports and every function they call. A forced function that can't be
/// compiled is an error listing the reasons; `"use js"` keeps a function (and its callers)
/// out of every island.
pub fn forced_islands(
    modules: &[SourceModule<'_>],
    facts: &Facts,
    extra: &dyn ExtraFacts,
) -> ForcedIslands {
    let mut out = ForcedIslands::default();
    if !modules.iter().any(|m| mentions_directive(m.source)) {
        return out;
    }
    let allocators: Vec<Allocator> = modules.iter().map(|_| Allocator::default()).collect();
    let world = World::build(&allocators, modules);

    for (module, directive) in &world.misplaced {
        if directive.placement == Placement::Wasm
            && let Some(module) = world.module(*module)
        {
            out.diagnostics.push(
                Diagnostic::error("only top-level function declarations can be forced into WASM")
                    .with_code("trioxide(wasm-ineligible)")
                    .with_file(module.path)
                    .with_label(directive.span, None),
            );
        }
    }
    let mut functions: Vec<_> = world.functions.values().collect();
    functions.sort_by_key(|info| (info.id.module, info.func.span.start));
    let mut ids: FxHashSet<String> = FxHashSet::default();
    for module in &world.modules {
        let mut roots = Vec::new();
        for info in functions.iter().filter(|f| f.id.module == module.id) {
            let Some(directive) = world.directive(info.id) else {
                continue;
            };
            let name = info.name.clone();
            if directive.placement == Placement::Js {
                out.decisions.push((
                    module.id,
                    decision(
                        name,
                        info.func.span,
                        Target::Js,
                        "marked \"use js\"",
                        None,
                        None,
                    ),
                ));
                continue;
            }
            let valid_name = name
                .as_deref()
                .filter(|n| names::is_abi_name(n) && *n != "ready");
            if never_called(module, facts, info.func) {
                // Nothing calls it, so nothing types its parameters; it never runs anyway.
                out.decisions.push((
                    module.id,
                    decision(
                        name,
                        info.func.span,
                        Target::Js,
                        "forced into WASM, but nothing calls it",
                        None,
                        None,
                    ),
                ));
                continue;
            }
            match (info.top_level && info.role == Role::Free, valid_name) {
                (true, Some(_)) => roots.push(info.id),
                (true, None) => out.diagnostics.push(
                    Diagnostic::error(format!(
                        "`{}` can't be a WASM island export: its name must be an ASCII \
                         identifier other than `ready`",
                        name.unwrap_or_default()
                    ))
                    .with_code("trioxide(wasm-ineligible)")
                    .with_file(module.path)
                    .with_label(directive.span, Some("forced into WASM here".into())),
                ),
                (false, _) if !directive.file => out.diagnostics.push(
                    Diagnostic::error(
                        "only top-level function declarations can be forced into WASM",
                    )
                    .with_code("trioxide(wasm-ineligible)")
                    .with_file(module.path)
                    .with_label(directive.span, None),
                ),
                (false, _) => {}
            }
        }
        if roots.is_empty() {
            continue;
        }
        let id = island_id(module.path, &mut ids);
        let label = module.path.file_name().map_or_else(
            || module.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let lowered = lower::lower_island(&world, facts, extra, &id, &label, &roots);
        for (root, reasons) in &lowered.failed {
            let info = &world.functions[root];
            let name = info.name.clone().unwrap_or_default();
            out.diagnostics.push(ineligible_diagnostic(
                &world,
                module.path,
                &name,
                info.func.span,
                reasons,
            ));
            let first = reasons.first();
            out.decisions.push((
                module.id,
                decision(
                    info.name.clone(),
                    info.func.span,
                    Target::Js,
                    &format!(
                        "forced into WASM but ineligible: {}",
                        first.map_or("unknown reason", |r| r.message.as_str())
                    ),
                    first
                        .filter(|r| r.function.module == module.id)
                        .map(|r| r.span),
                    None,
                ),
            ));
        }
        let Some(island) = lowered.island else {
            continue;
        };
        let exports: Vec<(FunctionId, String)> = island
            .exports
            .iter()
            .map(|export| (lowered.members[export.func.0], export.name.clone()))
            .collect();
        let bindings: Vec<ExportBinding> = exports
            .iter()
            .filter_map(|(function, name)| {
                let info = world.functions.get(function)?;
                let symbol = info.func.id.as_ref()?.symbol_id.get()?;
                let (exported_as, reassigned) =
                    world.module(function.module)?.exported_names(symbol);
                (!reassigned).then(|| ExportBinding {
                    name: name.clone(),
                    exported_as,
                })
            })
            .collect();
        let mut spec = emit::spec(&island);
        // The glue calls the JS version by the name the module exports it under.
        for export in &mut spec.exports {
            export.fallback_name = bindings
                .iter()
                .find(|b| b.name == export.name && !b.exported_as.contains(&export.name))
                .and_then(|b| b.exported_as.first().cloned());
        }
        let lib = emit::lib_rs(&island, &spec);
        let source = IslandSource::new(spec, lib);
        if let Err(error) = source.validate() {
            out.diagnostics.push(
                Diagnostic::error(format!(
                    "trioxide generated an invalid island (a bug): {error}"
                ))
                .with_code("trioxide(wasm-ineligible)")
                .with_file(module.path),
            );
            continue;
        }
        // The glue calls the JS version for overlapping arguments, and for object arguments
        // whose fields aren't plain data properties.
        let needs_fallback = island.exports.iter().any(|export| {
            let aggregates = export
                .params
                .iter()
                .filter(|(_, ty, _)| !ty.is_scalar())
                .count();
            aggregates > 1 && export.params.iter().any(|(_, _, mutated)| *mutated)
                || export
                    .params
                    .iter()
                    .any(|(_, ty, _)| matches!(ty, trioxide_wasm::AbiType::Struct { .. }))
        });
        for &member in &lowered.members {
            let Some(info) = world.functions.get(&member) else {
                continue;
            };
            let exported = exports.iter().any(|(f, _)| *f == member);
            let reason = if exported {
                "forced into WASM; exported by the island".to_string()
            } else {
                "compiled into the island as a callee (its JS version is kept too)".to_string()
            };
            out.decisions.push((
                member.module,
                decision(
                    info.name.clone(),
                    info.func.span,
                    Target::Wasm,
                    &reason,
                    None,
                    Some(id.clone()),
                ),
            ));
        }
        out.islands.push(GeneratedIsland {
            source,
            module: module.id,
            exports,
            bindings,
            members: lowered.members,
            needs_fallback,
            rir: island,
        });
    }
    out
}

fn decision(
    name: Option<String>,
    span: Span,
    target: Target,
    reason: &str,
    culprit: Option<Span>,
    island: Option<String>,
) -> FunctionDecision {
    FunctionDecision {
        name,
        span,
        target,
        reason: reason.to_string(),
        culprit,
        island,
        cost: None,
    }
}

/// Whether the analysis proves nothing calls `func`: it is unreachable, or a parameter never
/// received a value.
fn never_called(
    module: &world::Module<'_>,
    facts: &Facts,
    func: &oxc_ast::ast::Function<'_>,
) -> bool {
    let reachable = facts.reachable(module.node(func.node_id()));
    if !reachable.value && facts.is_usable(reachable.trust) {
        return true;
    }
    func.params.items.iter().any(|param| {
        param
            .pattern
            .get_binding_identifier()
            .and_then(|id| id.symbol_id.get())
            .is_some_and(|symbol| {
                let fact = facts.symbol_ty(module.symbol(symbol));
                facts.is_usable(fact.trust) && matches!(fact.value, Type::Never)
            })
    })
}

/// The error for a forced function that can't be compiled.
fn ineligible_diagnostic(
    world: &World<'_>,
    path: &Path,
    name: &str,
    span: Span,
    reasons: &[Reason],
) -> Diagnostic {
    let module = span_module(world, reasons, path);
    let mut diagnostic = Diagnostic::error(format!(
        "`{name}` is forced into WASM but can't be compiled: {}",
        reasons
            .first()
            .map_or("unknown reason", |r| r.message.as_str())
    ))
    .with_code("trioxide(wasm-ineligible)")
    .with_file(path);
    let mut elsewhere = Vec::new();
    for reason in reasons {
        let message = format!(
            "`{}` {}",
            function_name(world, reason.function),
            reason.message
        );
        if world.module(reason.function.module).map(|m| m.path) == Some(module) {
            diagnostic = diagnostic.with_label(reason.span, Some(message));
        } else if let Some(other) = world.module(reason.function.module) {
            let (line, column) = line_column(other.source, reason.span.start);
            elsewhere.push(format!(
                "{}:{line}:{column}: {message}",
                other.path.display()
            ));
        }
    }
    if diagnostic.labels.is_empty() {
        diagnostic = diagnostic.with_label(span, None);
    }
    let mut help = String::from(
        "only numbers, booleans, typed arrays, `number[]`, plain-data objects, local control \
         flow, arithmetic, `Math.*` and calls to other compilable functions can be compiled \
         (docs/PLAN.md §5); remove the directive to keep it in JS",
    );
    if !elsewhere.is_empty() {
        help.push_str("; also: ");
        help.push_str(&elsewhere.join("; "));
    }
    diagnostic.with_help(help)
}

fn span_module<'p>(_world: &World<'_>, _reasons: &[Reason], path: &'p Path) -> &'p Path {
    path
}

fn function_name(world: &World<'_>, function: FunctionId) -> String {
    world
        .functions
        .get(&function)
        .and_then(|info| info.name.clone())
        .unwrap_or_else(|| "constructor".into())
}

fn line_column(source: &str, offset: u32) -> (usize, usize) {
    let before = &source[..(offset as usize).min(source.len())];
    let line = before.matches('\n').count() + 1;
    let column = before
        .rfind('\n')
        .map_or(before.len(), |i| before.len() - i - 1)
        + 1;
    (line, column)
}

/// An island id from the module's file name, unique in the build.
fn island_id(path: &Path, taken: &mut FxHashSet<String>) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut base: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(56)
        .collect();
    if base.is_empty() {
        base = "island".into();
    }
    let mut id = base.clone();
    let mut n = 2;
    while taken.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    taken.insert(id.clone());
    id
}
