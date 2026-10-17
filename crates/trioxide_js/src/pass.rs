use std::collections::BTreeMap;

use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_semantic::{Scoping, SemanticBuilder};
use oxc_traverse::{Traverse, traverse_mut};
use trioxide_analysis::Facts;
use trioxide_common::{ModuleId, log::ModuleLog};

/// Which passes run, from `Options::passes` and `Options::trust_types`.
#[derive(Debug, Clone, Default)]
pub struct PassOptions {
    /// Per-pass switches by [`Pass::name`]. A pass not listed uses its own default.
    pub enabled: BTreeMap<String, bool>,
    /// `trustTypes`: whether `Declared` facts count as proof (PLAN D1).
    pub trust_types: bool,
}

impl PassOptions {
    /// Whether `pass` should run, given its default.
    pub fn is_enabled(&self, pass: &dyn Pass) -> bool {
        self.enabled
            .get(pass.name())
            .copied()
            .unwrap_or_else(|| pass.enabled_by_default())
    }
}

/// Supplies analysis facts to passes, and refreshes them after the AST changes.
///
/// Facts are keyed by `NodeId`s, which a rewrite invalidates. After any pass group changes a
/// module, the pass manager rebuilds the module's semantic data and calls
/// [`refresh`](Self::refresh) so the module's tables match the new AST (PLAN §2: "Analyses
/// must be cheap to recompute per module").
pub trait FactSource {
    /// The current facts.
    fn facts(&self) -> &Facts;

    /// Recomputes the facts for `module` from its rewritten AST. The default does nothing,
    /// which is only correct while no pass reads node-keyed facts after a rewrite.
    fn refresh(&mut self, module: ModuleId, program: &Program<'_>, scoping: &Scoping) {
        let _ = (module, program, scoping);
    }
}

impl FactSource for Facts {
    fn facts(&self) -> &Facts {
        self
    }
}

/// What a pass gets to work with.
pub struct PassContext<'a, 'f> {
    /// The arena the module's AST lives in. New nodes must be allocated here.
    pub allocator: &'a Allocator,
    /// The module being optimised.
    pub module: ModuleId,
    /// Analysis facts. Check trust with [`Facts::is_usable`] before relying on one.
    pub facts: &'f Facts,
    /// Where to record what fired and what was skipped, for `trioxide explain`.
    pub log: &'f mut ModuleLog,
    scoping: Option<Scoping>,
}

impl<'a> PassContext<'a, '_> {
    /// The module's current scoping (symbols, scopes, references).
    pub fn scoping(&self) -> &Scoping {
        self.scoping
            .as_ref()
            .expect("scoping is present outside traverse")
    }

    /// Mutable scoping, for passes that edit the AST without `oxc_traverse`.
    pub fn scoping_mut(&mut self) -> &mut Scoping {
        self.scoping
            .as_mut()
            .expect("scoping is present outside traverse")
    }

    /// Runs an `oxc_traverse` visitor over `program`, threading the scoping through it. This
    /// is how most passes should be written.
    pub fn traverse<State, T: Traverse<'a, State>>(
        &mut self,
        visitor: &mut T,
        program: &mut Program<'a>,
        state: State,
    ) {
        let scoping = self
            .scoping
            .take()
            .expect("scoping is present outside traverse");
        self.scoping = Some(traverse_mut(
            visitor,
            self.allocator,
            program,
            scoping,
            state,
        ));
    }
}

/// One optimisation pass (PLAN §4).
///
/// A pass rewrites one module's AST in place. Each must have a precondition stated in terms
/// of analysis facts, skip (and log why) when a fact is not usable, and come with snapshot and
/// differential fixtures.
pub trait Pass {
    /// The name used in `Options::passes` and in the decision log, e.g. `"fold"`.
    fn name(&self) -> &'static str;

    /// Whether the pass runs when the options don't mention it.
    fn enabled_by_default(&self) -> bool {
        true
    }

    /// Runs the pass. Returns whether it changed the AST.
    fn run<'a>(&mut self, program: &mut Program<'a>, ctx: &mut PassContext<'a, '_>) -> bool;
}

/// Passes that run together between semantic rebuilds.
pub struct PassGroup {
    /// A name for logs.
    pub name: &'static str,
    /// The passes, run in order.
    pub passes: Vec<Box<dyn Pass>>,
    /// Re-run the group until no pass changes anything (bounded by
    /// [`PassManager::max_iterations`]).
    pub fixpoint: bool,
}

/// Runs pass groups over a module.
///
/// Within a group, passes share one [`Scoping`] that they keep up to date (`oxc_traverse`
/// does this). Between groups, and between fixpoint iterations, if anything changed the
/// manager rebuilds the semantic data from scratch and refreshes the module's facts, so every
/// group starts from consistent tables.
pub struct PassManager {
    groups: Vec<PassGroup>,
    options: PassOptions,
    /// The most times a fixpoint group is repeated.
    pub max_iterations: usize,
}

impl PassManager {
    /// A manager with no passes.
    pub fn new(options: PassOptions) -> Self {
        Self {
            groups: Vec::new(),
            options,
            max_iterations: 8,
        }
    }

    /// The default pipeline for `options`. Currently empty: S4 and S5 register passes here.
    pub fn from_options(options: PassOptions) -> Self {
        Self::new(options)
    }

    /// Appends a group. Passes disabled by the options are dropped.
    pub fn add_group(&mut self, mut group: PassGroup) {
        group
            .passes
            .retain(|pass| self.options.is_enabled(pass.as_ref()));
        if !group.passes.is_empty() {
            self.groups.push(group);
        }
    }

    /// The options in effect.
    pub fn options(&self) -> &PassOptions {
        &self.options
    }

    /// Whether no pass will run.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// Runs every group over `program` and returns the up-to-date scoping.
    pub fn run<'a>(
        &mut self,
        allocator: &'a Allocator,
        program: &mut Program<'a>,
        scoping: Scoping,
        module: ModuleId,
        facts: &mut dyn FactSource,
        log: &mut ModuleLog,
    ) -> Scoping {
        let mut scoping = scoping;
        for group in &mut self.groups {
            let iterations = if group.fixpoint {
                self.max_iterations
            } else {
                1
            };
            for _ in 0..iterations {
                let mut ctx = PassContext {
                    allocator,
                    module,
                    facts: facts.facts(),
                    log: &mut *log,
                    scoping: Some(scoping),
                };
                let mut changed = false;
                for pass in &mut group.passes {
                    changed |= pass.run(program, &mut ctx);
                }
                scoping = ctx
                    .scoping
                    .take()
                    .expect("scoping is present outside traverse");
                if !changed {
                    break;
                }
                scoping = rebuild_scoping(program);
                facts.refresh(module, program, &scoping);
            }
        }
        scoping
    }
}

/// Builds fresh semantic data for `program`, with TS enum values evaluated (the transformer
/// needs them to inline `const enum` members).
pub fn rebuild_scoping(program: &Program<'_>) -> Scoping {
    SemanticBuilder::new()
        .with_enum_eval(true)
        .build(program)
        .semantic
        .into_scoping()
}

#[cfg(test)]
mod tests {
    use oxc_ast::ast::{Expression, NumericLiteral};
    use oxc_parser::Parser;
    use oxc_span::SourceType;
    use oxc_traverse::TraverseCtx;

    use super::*;

    /// Doubles every numeric literal once: enough to see the manager thread scoping through
    /// a traverse-based pass and rebuild between iterations.
    struct Double;

    impl<'a> Traverse<'a, ()> for Double {
        fn enter_expression(&mut self, expr: &mut Expression<'a>, _ctx: &mut TraverseCtx<'a, ()>) {
            if let Expression::NumericLiteral(lit) = expr {
                let NumericLiteral { value, .. } = &mut **lit;
                *value *= 2.0;
                lit.raw = None;
            }
        }
    }

    impl Pass for Double {
        fn name(&self) -> &'static str {
            "double"
        }

        fn run<'a>(&mut self, program: &mut Program<'a>, ctx: &mut PassContext<'a, '_>) -> bool {
            ctx.traverse(self, program, ());
            ctx.log.applied(self.name(), None);
            false
        }
    }

    fn run(options: PassOptions) -> (String, ModuleLog) {
        let allocator = Allocator::default();
        let mut program = Parser::new(&allocator, "let x: number = 21;", SourceType::ts())
            .parse()
            .program;
        let scoping = rebuild_scoping(&program);
        let mut manager = PassManager::new(options);
        manager.add_group(PassGroup {
            name: "test",
            passes: vec![Box::new(Double)],
            fixpoint: true,
        });
        let mut facts = Facts::new(false);
        let mut log = ModuleLog::default();
        manager.run(
            &allocator,
            &mut program,
            scoping,
            ModuleId::new(0),
            &mut facts,
            &mut log,
        );
        let code = oxc_codegen::Codegen::new().build(&program).code;
        (code, log)
    }

    #[test]
    fn runs_enabled_passes() {
        let (code, log) = run(PassOptions::default());
        assert_eq!(code, "let x: number = 42;\n");
        assert_eq!(log.passes.len(), 1);
    }

    #[test]
    fn skips_disabled_passes() {
        let options = PassOptions {
            enabled: [("double".to_string(), false)].into(),
            trust_types: false,
        };
        let (code, log) = run(options);
        assert_eq!(code, "let x: number = 21;\n");
        assert_eq!(log.passes, Vec::new());
    }
}
