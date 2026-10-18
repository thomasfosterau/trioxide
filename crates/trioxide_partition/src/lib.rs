//! Decides, per function, whether code stays JS or becomes part of a WASM island (PLAN §5).
//!
//! [`Partitioner::partition_sources`] runs the whole pipeline:
//!
//! 1. Source directives (`"use wasm"`, `"use js"`, `/** @trioxide wasm|js */`) and config
//!    overrides (`overrides` globs in `trioxide.json`) place functions first; they win over
//!    the mode.
//! 2. In `auto` and `wasm-preferred` mode, every other top-level function declaration is
//!    checked by the Rust backend ([`eligibility::candidates`]); the eligible ones are
//!    clustered over the call graph ([`eligibility::clusters`]).
//! 3. Each cluster is scored by the [`CostModel`]; the ones whose `benefit − cost` beats the
//!    mode's threshold become islands, exporting the members JS calls.
//! 4. The Rust backend lowers forced and chosen functions together, one island per module
//!    ([`trioxide_rust::islands`]).
//!
//! Every function considered gets a verdict with its reason and, when it was scored, the
//! cost-model numbers, for `trioxide explain`.

mod cost;
mod metrics;

use std::path::{Path, PathBuf};

use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use trioxide_analysis::{CallEdge, Facts};
use trioxide_common::{
    Diagnostic, ModuleId, Span,
    log::{CostEstimate, FunctionDecision, RuleFailure, Target, number},
};
pub use trioxide_rust::{ExportBinding, Placement, SourceModule};
use trioxide_rust::{
    IslandRequest, NoExtraFacts,
    eligibility::{self, Candidate, Eligibility},
};
use trioxide_types::FunctionId;
use trioxide_wasm::{IslandId, IslandSource};

pub use crate::cost::CostModel;
use crate::metrics::Metrics;

/// How aggressively to move code to WASM (`Options::mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PartitionMode {
    /// Never create islands on its own; only directives and config overrides do.
    JsOnly,
    /// Create islands the cost model says are profitable.
    #[default]
    Auto,
    /// Like `Auto`, with the cost model's lower `wasmPreferredThreshold`.
    WasmPreferred,
}

/// Config overrides for the files matching some globs (`overrides` in `trioxide.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOverride {
    /// Glob patterns, relative to the project root (`*`, `**`, `?`, `[…]`, `{a,b}`).
    pub files: Vec<String>,
    /// Where every top-level function of a matching file goes, as a file directive would
    /// put it. Directives in the file win.
    pub placement: Placement,
}

/// Inputs to partitioning that come from options rather than analysis.
#[derive(Debug, Clone, Default)]
pub struct PartitionOptions {
    /// The partition mode.
    pub mode: PartitionMode,
    /// Whether WASM output is enabled at all (`--no-wasm` turns it off).
    pub wasm: bool,
    /// The cost model's weights and thresholds.
    pub cost: CostModel,
    /// Per-file overrides, in order (later wins).
    pub overrides: Vec<FileOverride>,
    /// The project root, which override globs are relative to.
    pub root: PathBuf,
}

/// A planned island: the functions that compile together.
#[derive(Debug, Clone, PartialEq)]
pub struct IslandPlan {
    /// The island's id.
    pub id: IslandId,
    /// Its member functions.
    pub functions: Vec<FunctionId>,
    /// The modules whose code calls into it (they import its glue).
    pub importers: Vec<ModuleId>,
    /// The module whose functions it exports.
    pub module: Option<ModuleId>,
    /// The functions it exports to JS, with their names.
    pub exports: Vec<(FunctionId, String)>,
    /// The exports JS calls may be pointed at (their bindings are never reassigned).
    pub bindings: Vec<ExportBinding>,
    /// The generated crate, once the Rust backend has lowered it.
    pub source: Option<IslandSource>,
    /// Some export may be called with overlapping arguments it writes to, so the glue needs
    /// the JS versions as a `fallback` (`docs/ABI.md`, "Aliasing").
    pub needs_fallback: bool,
}

/// The partitioner's output.
///
/// Per-function verdicts (with reasons) go into each module's
/// [`ModuleLog`](trioxide_common::log::ModuleLog) for `explain`; this holds what later stages
/// act on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Partition {
    /// Islands to build. Every function not in an island stays JS.
    pub islands: Vec<IslandPlan>,
    /// Per-function verdicts, for each module's decision log.
    pub decisions: Vec<(ModuleId, FunctionDecision)>,
    /// Problems with directives (a forced function that can't be compiled is an error).
    pub diagnostics: Vec<Diagnostic>,
}

impl Partition {
    /// Whether everything stays JS.
    pub fn is_all_js(&self) -> bool {
        self.islands.is_empty()
    }
}

/// Splits a program into JS and WASM islands.
#[derive(Debug, Clone, Default)]
pub struct Partitioner {
    options: PartitionOptions,
}

/// What the automatic part decided.
#[derive(Default)]
struct Choice {
    /// Functions to export from islands.
    automatic: Vec<FunctionId>,
    /// Verdicts for candidates the islands' own decisions won't cover.
    decisions: Vec<(ModuleId, FunctionDecision)>,
    /// Cost-model numbers per scored function, by module and span.
    scores: FxHashMap<(ModuleId, Span), CostEstimate>,
}

impl Partitioner {
    /// A partitioner with the given options.
    pub fn new(options: PartitionOptions) -> Self {
        Self { options }
    }

    /// The options in effect.
    pub fn options(&self) -> &PartitionOptions {
        &self.options
    }

    /// The threshold `benefit − cost` must beat in the current mode.
    pub fn threshold(&self) -> f64 {
        match self.options.mode {
            PartitionMode::WasmPreferred => self.options.cost.wasm_preferred_threshold,
            PartitionMode::Auto | PartitionMode::JsOnly => self.options.cost.threshold,
        }
    }

    /// Partitions the program described by `facts` alone. Without sources nothing can be
    /// lowered, so everything stays JS; see [`partition_sources`](Self::partition_sources).
    pub fn partition(&self, facts: &Facts) -> Partition {
        let _ = facts;
        Partition::default()
    }

    /// Partitions the program in `modules` (see the crate docs): one island per module with
    /// forced or chosen functions, lowered by the Rust backend. Nothing when WASM is off.
    pub fn partition_sources(&self, facts: &Facts, modules: &[SourceModule<'_>]) -> Partition {
        if !self.options.wasm {
            return Partition::default();
        }
        let overrides = self.module_overrides(modules);
        let choice = if self.options.mode == PartitionMode::JsOnly {
            Choice::default()
        } else {
            self.choose(facts, modules, &overrides)
        };
        let generated = trioxide_rust::islands(
            modules,
            facts,
            &NoExtraFacts,
            &IslandRequest {
                overrides: &overrides,
                automatic: &choice.automatic,
            },
        );
        let islands = generated
            .islands
            .into_iter()
            .map(|island| {
                let mut importers = vec![island.module];
                for edge in facts.call_graph().edges() {
                    let exported = island.exports.iter().any(|(f, _)| *f == edge.callee);
                    if exported
                        && !island.members.contains(&edge.caller)
                        && !importers.contains(&edge.caller.module)
                    {
                        importers.push(edge.caller.module);
                    }
                }
                IslandPlan {
                    id: island.source.spec.id.clone(),
                    functions: island.members,
                    importers,
                    module: Some(island.module),
                    exports: island.exports,
                    bindings: island.bindings,
                    source: Some(island.source),
                    needs_fallback: island.needs_fallback,
                }
            })
            .collect();

        let mut decisions = generated.decisions;
        let decided = |decisions: &[(ModuleId, FunctionDecision)], module, span| {
            decisions
                .iter()
                .any(|(m, d): &(ModuleId, FunctionDecision)| *m == module && d.span == span)
        };
        for (module, decision) in choice.decisions {
            if !decided(&decisions, module, decision.span) {
                decisions.push((module, decision));
            }
        }
        for (module, decision) in &mut decisions {
            if let Some(estimate) = choice.scores.get(&(*module, decision.span)) {
                decision.cost = Some(*estimate);
            }
        }
        decisions.sort_by_key(|(module, decision)| (*module, decision.span.start));
        // A callee compiled into several islands is one function: one entry naming them all.
        decisions.dedup_by(|(module, later), (kept_module, kept)| {
            let same = module == kept_module && later.span == kept.span;
            if same && let (Some(island), Some(islands)) = (&later.island, &mut kept.island) {
                islands.push_str(", ");
                islands.push_str(island);
            }
            same
        });
        Partition {
            islands,
            decisions,
            diagnostics: generated.diagnostics,
        }
    }

    /// The placement config overrides give each module (the last matching override wins).
    fn module_overrides(&self, modules: &[SourceModule<'_>]) -> Vec<(ModuleId, Placement)> {
        if self.options.overrides.is_empty() {
            return Vec::new();
        }
        modules
            .iter()
            .filter_map(|module| {
                let relative = relative_path(&self.options.root, module.path);
                self.options
                    .overrides
                    .iter()
                    .rev()
                    .find(|o| {
                        o.files
                            .iter()
                            .any(|glob| fast_glob::glob_match(glob.as_str(), relative.as_str()))
                    })
                    .map(|o| (module.id, o.placement))
            })
            .collect()
    }

    /// Picks the clusters of eligible functions the cost model likes.
    fn choose(
        &self,
        facts: &Facts,
        modules: &[SourceModule<'_>],
        overrides: &[(ModuleId, Placement)],
    ) -> Choice {
        let mut choice = Choice::default();
        let candidates = eligibility::candidates(modules, facts, &NoExtraFacts, overrides);
        let eligible: Vec<FunctionId> = candidates
            .iter()
            .filter(|c| c.eligibility == Some(Eligibility::Eligible))
            .map(|c| c.function)
            .collect();
        let threshold = self.threshold();
        let metrics = if eligible.is_empty() {
            Metrics::default()
        } else {
            Metrics::measure(modules, facts, &self.options.cost)
        };
        let mut chosen = Vec::new();
        let span_of: FxHashMap<FunctionId, Span> =
            candidates.iter().map(|c| (c.function, c.span)).collect();
        // The calls the facade points at an island's glue: direct calls of an exported,
        // never-reassigned binding, from its own module or one importing it directly.
        let exported: FxHashSet<FunctionId> = candidates
            .iter()
            .filter(|c| c.exported)
            .map(|c| c.function)
            .collect();
        let imports: FxHashMap<ModuleId, &[ModuleId]> =
            modules.iter().map(|m| (m.id, m.imports)).collect();
        let reaches = |edge: &CallEdge| {
            metrics.direct.contains(&edge.site)
                && exported.contains(&edge.callee)
                && (edge.caller.module == edge.callee.module
                    || imports
                        .get(&edge.caller.module)
                        .is_some_and(|imports| imports.contains(&edge.callee.module)))
        };
        for cluster in eligibility::clusters(facts, &eligible) {
            let score = self
                .options
                .cost
                .score(&cluster, facts, &metrics, threshold, &reaches);
            let profitable = score.estimate.profitable();
            for &function in &cluster {
                choice
                    .scores
                    .insert((function.module, span_of[&function]), score.estimate);
                if profitable {
                    chosen.push(function);
                }
            }
            if profitable {
                choice.automatic.extend(score.entries);
            }
        }
        for candidate in &candidates {
            if candidate.directive.is_some() {
                continue;
            }
            let module = candidate.function.module;
            let decision = if candidate.never_called {
                verdict(candidate, "nothing calls it, so it stays JS".into())
            } else {
                match &candidate.eligibility {
                    Some(Eligibility::Ineligible(reasons)) => {
                        ineligible(candidate, reasons, modules, facts)
                    }
                    Some(Eligibility::Eligible) => {
                        let Some(estimate) = choice.scores.get(&(module, candidate.span)) else {
                            continue;
                        };
                        if estimate.calls == 0.0 {
                            verdict(
                                candidate,
                                "stays JS: no call can reach an island (only direct calls of an \
                                 exported function from its module or a module importing it \
                                 can; the host, callbacks and re-exports call the JS version)"
                                    .into(),
                            )
                        } else if chosen.contains(&candidate.function) {
                            verdict(
                                candidate,
                                "chosen by the cost model, but only island functions call it"
                                    .into(),
                            )
                        } else {
                            verdict(
                                candidate,
                                format!(
                                    "the cost model keeps it in JS: benefit − cost = {} ≤ \
                                     threshold {}",
                                    number(estimate.net()),
                                    number(estimate.threshold)
                                ),
                            )
                        }
                    }
                    None => continue,
                }
            };
            choice.decisions.push((module, decision));
        }
        choice
    }
}

/// A "stays JS" verdict on `candidate`.
fn verdict(candidate: &Candidate, reason: String) -> FunctionDecision {
    FunctionDecision {
        name: Some(candidate.name.clone()),
        span: candidate.span,
        target: Target::Js,
        reason,
        culprit: None,
        island: None,
        cost: None,
        directive: None,
        failures: Vec::new(),
    }
}

/// The verdict on a candidate the Rust backend can't compile, listing every reason.
fn ineligible(
    candidate: &Candidate,
    reasons: &[trioxide_rust::Reason],
    modules: &[SourceModule<'_>],
    facts: &Facts,
) -> FunctionDecision {
    let first = reasons.first();
    let mut decision = verdict(
        candidate,
        format!(
            "ineligible: {}",
            first.map_or("unknown reason", |r| r.message.as_str())
        ),
    );
    decision.culprit = first
        .filter(|r| r.function.module == candidate.function.module)
        .map(|r| r.span);
    decision.failures = reasons
        .iter()
        .map(|reason| RuleFailure {
            message: reason.message.clone(),
            span: reason.span,
            function: (reason.function != candidate.function).then(|| {
                facts
                    .function(reason.function)
                    .and_then(|f| f.name.clone())
                    .unwrap_or_else(|| "(anonymous)".into())
            }),
            path: (reason.function.module != candidate.function.module)
                .then(|| modules.iter().find(|m| m.id == reason.function.module))
                .flatten()
                .map(|m| m.path.to_path_buf()),
        })
        .collect();
    decision
}

/// `path` relative to `root`, with `/` separators, for matching globs.
fn relative_path(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_sources_everything_stays_js() {
        let partitioner = Partitioner::new(PartitionOptions {
            mode: PartitionMode::WasmPreferred,
            wasm: true,
            ..PartitionOptions::default()
        });
        assert!(partitioner.partition(&Facts::new(false)).is_all_js());
    }

    #[test]
    fn thresholds_follow_the_mode() {
        let mut options = PartitionOptions::default();
        let auto = Partitioner::new(options.clone()).threshold();
        options.mode = PartitionMode::WasmPreferred;
        assert!(Partitioner::new(options).threshold() < auto);
    }

    #[test]
    fn override_globs_match_relative_paths() {
        assert_eq!(
            relative_path(Path::new("/p"), Path::new("/p/src/dom/a.ts")),
            "src/dom/a.ts"
        );
        assert!(fast_glob::glob_match("src/dom/**", "src/dom/a.ts"));
        assert!(fast_glob::glob_match(
            "**/*.kernel.ts",
            "src/x/blur.kernel.ts"
        ));
        assert!(!fast_glob::glob_match("src/dom/**", "src/kernels/a.ts"));
    }
}
