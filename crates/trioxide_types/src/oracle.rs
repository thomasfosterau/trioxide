use std::path::Path;

use trioxide_common::{Diagnostic, ModuleId, NodeId, Span};

use crate::ty::{Signature, Type};

/// One module's source as the oracle should see it.
#[derive(Debug, Clone, Copy)]
pub struct SourceText<'a> {
    /// The module's id in the project graph; later queries name modules by it.
    pub module: ModuleId,
    /// Absolute path on disk (or the virtual path the project uses for it).
    pub path: &'a Path,
    /// The full TypeScript source. Spans in queries are UTF-8 byte offsets into this text.
    pub source: &'a str,
}

/// Everything an oracle needs to build its program.
#[derive(Debug, Clone, Copy)]
pub struct OracleInput<'a> {
    /// The project root.
    pub root: &'a Path,
    /// The `tsconfig.json` the project uses, if any.
    pub tsconfig: Option<&'a Path>,
    /// Every module in the graph. The oracle may read other files (e.g. `.d.ts` in
    /// `node_modules`) itself.
    pub modules: &'a [SourceText<'a>],
    /// Whether to collect TypeScript's own errors (`Options::check`).
    pub check: bool,
}

/// Answers questions about TypeScript types: the seam between trioxide and a TS checker.
///
/// The facade calls [`load`](Self::load) once per build with the whole module graph, then
/// [`update`](Self::update) whenever a module changes (Vite dev). Analysis then asks
/// questions by `(module, span)`: spans are UTF-8 byte offsets of an oxc AST node, which a
/// tsrs-backed oracle matches to tsrs nodes (PLAN §3 "Span correlation").
///
/// # Contract
///
/// - Answers describe what TypeScript *declares*. They are never more than `Declared`; the
///   analysis decides what is `Proven` (PLAN D1). An oracle must not guess: when a span
///   matches no checker node, or the type has no faithful trioxide equivalent, answer `None`.
/// - Queries take `&mut self` because tsrs checkers do. Implementations own their checker
///   state and must not let types from one checker mix with another's.
/// - Every query has a default implementation that knows nothing, so new queries can be
///   added without breaking existing oracles.
///
/// S2 implements this over tsrs; [`NoopTypeOracle`] is the stand-in until then and the
/// implementation used when the `tsrs` feature is off.
pub trait TypeOracle: Send {
    /// A short name for logs and reports, e.g. `"tsrs"` or `"noop"`.
    fn name(&self) -> &'static str;

    /// Builds the oracle's program over `input`. Returns configuration and (with
    /// `input.check`) type errors. Calling it again replaces the previous program.
    fn load(&mut self, input: &OracleInput<'_>) -> Vec<Diagnostic> {
        let _ = input;
        Vec::new()
    }

    /// Replaces one module's source after `load`. Implementations may rebuild lazily.
    fn update(&mut self, module: SourceText<'_>) {
        let _ = module;
    }

    /// The type of the expression, identifier or declaration at `span` in `module`.
    fn type_at(&mut self, module: ModuleId, span: Span) -> Option<Type> {
        let _ = (module, span);
        None
    }

    /// Where the symbol referenced at `span` is declared: the module and the declaration
    /// name's span. Imports resolve through to the exporting module.
    fn declaration_of(&mut self, module: ModuleId, span: Span) -> Option<(ModuleId, Span)> {
        let _ = (module, span);
        None
    }

    /// The signature TypeScript resolved for the call or `new` expression at `span`
    /// (overloads and generics already applied).
    fn resolved_signature(&mut self, module: ModuleId, span: Span) -> Option<Signature> {
        let _ = (module, span);
        None
    }

    /// TypeScript's errors for `module`; empty unless `check` was set on `load`.
    fn diagnostics(&mut self, module: ModuleId) -> Vec<Diagnostic> {
        let _ = module;
        Vec::new()
    }

    /// The declared type of the symbol referenced or declared at `span`: what TypeScript says
    /// every value of the binding has, without the flow narrowing [`type_at`](Self::type_at)
    /// applies. Imports resolve to the exported binding.
    fn symbol_type_at(&mut self, module: ModuleId, span: Span) -> Option<Type> {
        let _ = (module, span);
        None
    }

    /// Facts about TypeScript's type at `span` that the [`Type`] lattice does not keep, such as
    /// whether it is `any`.
    fn type_flags_at(&mut self, module: ModuleId, span: Span) -> Option<TsTypeFlags> {
        let _ = (module, span);
        None
    }

    /// TypeScript's rendering of the type at `span`, e.g. `"number[]"`. For debugging and
    /// reports only: never parse it.
    fn type_string_at(&mut self, module: ModuleId, span: Span) -> Option<String> {
        let _ = (module, span);
        None
    }

    /// Tells the oracle which oxc nodes declare classes and functions in `module`, so that
    /// [`Type::Instance`] and [`Type::Function`] answers can name them. Each entry is the span
    /// of the declaration's name (or of the whole node when it has none) and its node id.
    /// Replaces earlier registrations for `module`.
    fn register_declarations(&mut self, module: ModuleId, declarations: &[(Span, NodeId)]) {
        let _ = (module, declarations);
    }

    /// Whether `span` matches a checker node in `module` at all. Used to measure how well oxc
    /// and checker spans line up.
    fn matches_node(&mut self, module: ModuleId, span: Span) -> bool {
        let _ = (module, span);
        false
    }
}

/// Facts about a TypeScript type that [`Type`] does not represent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[expect(clippy::struct_excessive_bools, reason = "independent flags")]
pub struct TsTypeFlags {
    /// `any`, or a union containing it. Values of such a type can be anything.
    pub any: bool,
    /// `unknown`.
    pub unknown: bool,
    /// A literal type (`1`, `"a"`, `true`) or a union of them.
    pub literal: bool,
    /// A type parameter or a type built from one (indexed access, conditional).
    pub generic: bool,
    /// Includes `null` or `undefined`.
    pub nullable: bool,
}

/// An oracle that knows nothing: every type is unknown.
///
/// Used until S2's tsrs oracle lands and whenever trioxide is built without the `tsrs`
/// feature. With it, analysis can still prove facts from literals and flow, but nothing from
/// TS annotations.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopTypeOracle;

impl TypeOracle for NoopTypeOracle {
    fn name(&self) -> &'static str {
        "noop"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_knows_nothing() {
        let mut oracle = NoopTypeOracle;
        let module = ModuleId::new(0);
        let input = OracleInput {
            root: Path::new("/"),
            tsconfig: None,
            modules: &[],
            check: true,
        };
        assert_eq!(oracle.load(&input), Vec::new());
        assert_eq!(oracle.type_at(module, Span::new(0, 1)), None);
        assert_eq!(oracle.declaration_of(module, Span::new(0, 1)), None);
        assert_eq!(oracle.resolved_signature(module, Span::new(0, 1)), None);
        assert_eq!(oracle.diagnostics(module), Vec::new());
    }
}
