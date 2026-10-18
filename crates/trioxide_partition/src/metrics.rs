//! Static estimates the cost model scores with: per function, its compute weight per call and
//! the calls it makes; per call site, how often it runs per run of its function.
//!
//! Modules are parsed the way the analysis parses them, so node ids match the [`Facts`].

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ArrowFunctionExpression, AssignmentExpression, AssignmentOperator, AssignmentTarget,
    BinaryExpression, CallExpression, ComputedMemberExpression, DoWhileStatement, Expression,
    ForInStatement, ForOfStatement, ForStatement, ForStatementInit, Function, NewExpression,
    ReturnStatement, UnaryExpression, UpdateExpression, WhileStatement,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_semantic::{Scoping, SemanticBuilder};
use oxc_syntax::{operator::BinaryOperator, scope::ScopeFlags};
use rustc_hash::FxHashMap;
use trioxide_analysis::{Facts, Trust};
use trioxide_common::{ModuleId, NodeRef, SymbolRef};
use trioxide_types::{FunctionId, Type, TypedArrayKind};

use crate::{CostModel, SourceModule};

/// What crosses the boundary for one parameter or result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Marshal {
    /// Numbers and booleans pass directly.
    Scalar,
    /// A typed array with elements of this many bytes, copied in (and out when written).
    Typed(f64),
    /// A `number[]`, converted element by element.
    Numbers,
    /// A plain-data object: its fields are copied.
    Object(f64),
}

/// One function's estimates.
#[derive(Debug, Clone, Default)]
pub(crate) struct FunctionMetrics {
    /// Weighted operations per call, loops multiplied in; calls excluded.
    pub work: f64,
    /// Call sites in its body (not in nested functions), with how often each runs per call.
    pub calls: Vec<(NodeRef, f64)>,
    /// Its parameters, with whether its body writes their elements or fields.
    pub params: Vec<(SymbolRef, bool)>,
    /// Arrays it returns, if any.
    pub returns: Option<Marshal>,
}

/// Estimates for a whole program.
#[derive(Debug, Default)]
pub(crate) struct Metrics {
    pub functions: FxHashMap<FunctionId, FunctionMetrics>,
    /// How often each call site runs per run of the unit containing it.
    pub sites: FxHashMap<NodeRef, f64>,
}

impl Metrics {
    /// Walks every module.
    pub fn measure(modules: &[SourceModule<'_>], facts: &Facts, model: &CostModel) -> Self {
        let mut metrics = Self::default();
        for module in modules {
            let allocator = Allocator::default();
            let parsed = Parser::new(&allocator, module.source, module.source_type).parse();
            let program = allocator.alloc(parsed.program);
            let semantic = SemanticBuilder::new()
                .with_enum_eval(true)
                .with_build_nodes(true)
                .build(program)
                .semantic;
            let mut walker = Walker {
                module: module.id,
                facts,
                model,
                scoping: semantic.scoping(),
                frames: vec![Frame {
                    id: None,
                    freq: 1.0,
                    metrics: FunctionMetrics::default(),
                }],
                out: &mut metrics,
            };
            walker.visit_program(program);
        }
        metrics
    }

    /// The bytes copied across the boundary per call of `function`.
    pub fn bytes(&self, function: FunctionId, facts: &Facts, model: &CostModel) -> f64 {
        let Some(metrics) = self.functions.get(&function) else {
            return 0.0;
        };
        let elements = model.trips;
        let size = |marshal: Marshal, written: bool| {
            let copies = if written { 2.0 } else { 1.0 };
            match marshal {
                Marshal::Scalar => 0.0,
                Marshal::Typed(bytes) => elements * bytes * copies,
                Marshal::Numbers => elements * 8.0 * copies * model.number_array,
                Marshal::Object(fields) => fields * 8.0 * copies,
            }
        };
        let params: f64 = metrics
            .params
            .iter()
            .map(|&(param, written)| size(marshal_of(facts.symbol_ty(param).value), written))
            .sum();
        params + metrics.returns.map_or(0.0, |marshal| size(marshal, false))
    }
}

/// How a value of type `ty` crosses the boundary.
fn marshal_of(ty: &Type) -> Marshal {
    match ty {
        Type::TypedArray(kind) => Marshal::Typed(element_bytes(*kind)),
        Type::Array(_) | Type::Tuple(_) => Marshal::Numbers,
        Type::Object(shape) => {
            #[expect(clippy::cast_precision_loss, reason = "field counts are small")]
            let fields = shape.properties.len() as f64;
            Marshal::Object(fields)
        }
        Type::Instance(_) => Marshal::Object(2.0),
        Type::Union(members) => members
            .iter()
            .map(marshal_of)
            .find(|m| *m != Marshal::Scalar)
            .unwrap_or(Marshal::Scalar),
        _ => Marshal::Scalar,
    }
}

fn element_bytes(kind: TypedArrayKind) -> f64 {
    match kind {
        TypedArrayKind::Int8 | TypedArrayKind::Uint8 | TypedArrayKind::Uint8Clamped => 1.0,
        TypedArrayKind::Int16 | TypedArrayKind::Uint16 => 2.0,
        TypedArrayKind::Int32 | TypedArrayKind::Uint32 | TypedArrayKind::Float32 => 4.0,
        TypedArrayKind::Float64 | TypedArrayKind::BigInt64 | TypedArrayKind::BigUint64 => 8.0,
    }
}

struct Frame {
    id: Option<FunctionId>,
    /// How often the code being walked runs per call of the function.
    freq: f64,
    metrics: FunctionMetrics,
}

struct Walker<'w, 's> {
    module: ModuleId,
    facts: &'w Facts,
    model: &'w CostModel,
    scoping: &'s Scoping,
    frames: Vec<Frame>,
    out: &'w mut Metrics,
}

impl Walker<'_, '_> {
    fn node(&self, node: oxc_syntax::node::NodeId) -> NodeRef {
        NodeRef {
            module: self.module,
            node,
        }
    }

    fn frame(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("the program frame")
    }

    fn add(&mut self, weight: f64) {
        let frame = self.frame();
        frame.metrics.work += weight * frame.freq;
    }

    /// Whether the analysis knows the value of `expression` is an integer.
    fn integer(&self, expression: &Expression<'_>) -> bool {
        let fact = self.facts.range(self.node(expression.node_id()));
        fact.trust > Trust::Unknown && fact.value.int
    }

    fn typed_array(&self, expression: &Expression<'_>) -> bool {
        let fact = self
            .facts
            .ty(self.node(expression.get_inner_expression().node_id()));
        matches!(fact.value, Type::TypedArray(_))
    }

    /// The symbol `expression` reads, if it is a plain identifier.
    fn symbol(&self, expression: &Expression<'_>) -> Option<SymbolRef> {
        let Expression::Identifier(id) = expression.get_inner_expression() else {
            return None;
        };
        let symbol = self
            .scoping
            .get_reference(id.reference_id.get()?)
            .symbol_id()?;
        Some(SymbolRef {
            module: self.module,
            symbol,
        })
    }

    /// A finite bound the analysis knows for `expression`: its range's `max` (or `min`).
    fn bound(&self, expression: &Expression<'_>, max: bool) -> Option<f64> {
        let fact = self
            .facts
            .range(self.node(expression.get_inner_expression().node_id()));
        let range = fact.value.range;
        let value = if max { range.max } else { range.min };
        (fact.trust > Trust::Unknown && value.is_finite() && value.abs() < f64::from(1 << 24))
            .then_some(value)
    }

    /// The trip count of the loop `node` with condition `test`: when the test compares the
    /// loop's counter (an induction variable with a constant step) with a value of known
    /// range, from where the counter starts (`start`, else its range at the loop head);
    /// otherwise the model's default.
    fn trips(
        &self,
        node: oxc_syntax::node::NodeId,
        test: Option<&Expression<'_>>,
        start: Option<(SymbolRef, &Expression<'_>)>,
    ) -> f64 {
        let estimate = || {
            let loop_facts = self.facts.loop_facts(self.node(node))?;
            let Expression::BinaryExpression(test) = test?.get_inner_expression() else {
                return None;
            };
            let (counter, operator, limit) =
                match (self.symbol(&test.left), self.symbol(&test.right)) {
                    (Some(left), _) => (left, test.operator, &test.right),
                    (None, Some(right)) => (right, flip(test.operator)?, &test.left),
                    (None, None) => return None,
                };
            let induction = loop_facts
                .induction
                .iter()
                .find(|i| i.symbol == counter && i.counter)?;
            let step = induction.step?;
            let up = step > 0.0;
            let from = match start {
                Some((symbol, init)) if symbol == counter => self.bound(init, !up)?,
                _ => {
                    let range = induction.range.value.range;
                    let from = if up { range.min } else { range.max };
                    (induction.range.trust > Trust::Unknown && from.is_finite()).then_some(from)?
                }
            };
            let (to, inclusive) = match (operator, up) {
                (BinaryOperator::LessThan, true) => (self.bound(limit, true)?, false),
                (BinaryOperator::LessEqualThan, true) => (self.bound(limit, true)?, true),
                (BinaryOperator::GreaterThan, false) => (self.bound(limit, false)?, false),
                (BinaryOperator::GreaterEqualThan, false) => (self.bound(limit, false)?, true),
                _ => return None,
            };
            let distance = (to - from) / step;
            let trips = if inclusive {
                distance.floor() + 1.0
            } else {
                distance.ceil()
            };
            Some(trips.max(0.0))
        };
        estimate().unwrap_or(self.model.trips)
    }

    /// Walks a loop body (with its test and update) at `trips` times the current frequency.
    fn looped(&mut self, trips: f64, walk: impl FnOnce(&mut Self)) {
        let outer = self.frame().freq;
        self.frame().freq = outer * trips;
        self.add(self.model.iteration);
        walk(self);
        self.frame().freq = outer;
    }

    fn enter(&mut self, id: FunctionId, params: Vec<SymbolRef>) {
        self.frames.push(Frame {
            id: Some(id),
            freq: 1.0,
            metrics: FunctionMetrics {
                params: params.into_iter().map(|p| (p, false)).collect(),
                ..FunctionMetrics::default()
            },
        });
    }

    fn leave(&mut self) {
        let frame = self.frames.pop().expect("entered");
        if let Some(id) = frame.id {
            self.out.functions.insert(id, frame.metrics);
        }
    }

    /// Records a call site.
    fn site(&mut self, node: oxc_syntax::node::NodeId) {
        let site = self.node(node);
        let frame = self.frames.last_mut().expect("the program frame");
        frame.metrics.calls.push((site, frame.freq));
        self.out.sites.insert(site, frame.freq);
    }

    /// Marks the parameter `expression` names (if it does) as written through.
    fn written(&mut self, expression: &Expression<'_>) {
        let Expression::Identifier(id) = expression.get_inner_expression() else {
            return;
        };
        let Some(symbol) = id
            .reference_id
            .get()
            .and_then(|r| self.scoping.get_reference(r).symbol_id())
        else {
            return;
        };
        let symbol = SymbolRef {
            module: self.module,
            symbol,
        };
        if let Some(param) = self
            .frame()
            .metrics
            .params
            .iter_mut()
            .find(|(p, _)| *p == symbol)
        {
            param.1 = true;
        }
    }

    fn params(&self, function: &oxc_ast::ast::FormalParameters<'_>) -> Vec<SymbolRef> {
        function
            .items
            .iter()
            .filter_map(|param| param.pattern.get_binding_identifier()?.symbol_id.get())
            .map(|symbol| SymbolRef {
                module: self.module,
                symbol,
            })
            .collect()
    }
}

impl<'a> Visit<'a> for Walker<'_, '_> {
    fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
        let params = self.params(&function.params);
        self.enter(self.node(function.node_id()), params);
        walk::walk_function(self, function, flags);
        self.leave();
    }

    fn visit_arrow_function_expression(&mut self, arrow: &ArrowFunctionExpression<'a>) {
        let params = self.params(&arrow.params);
        self.enter(self.node(arrow.node_id()), params);
        walk::walk_arrow_function_expression(self, arrow);
        self.leave();
    }

    fn visit_for_statement(&mut self, it: &ForStatement<'a>) {
        if let Some(init) = &it.init {
            self.visit_for_statement_init(init);
        }
        let start = match &it.init {
            Some(ForStatementInit::VariableDeclaration(declaration)) => {
                declaration.declarations.first().and_then(|d| {
                    let symbol = d.id.get_binding_identifier()?.symbol_id.get()?;
                    Some((
                        SymbolRef {
                            module: self.module,
                            symbol,
                        },
                        d.init.as_ref()?,
                    ))
                })
            }
            _ => None,
        };
        let trips = self.trips(it.node_id(), it.test.as_ref(), start);
        self.looped(trips, |this| {
            if let Some(test) = &it.test {
                this.visit_expression(test);
            }
            if let Some(update) = &it.update {
                this.visit_expression(update);
            }
            this.visit_statement(&it.body);
        });
    }

    fn visit_while_statement(&mut self, it: &WhileStatement<'a>) {
        let trips = self.trips(it.node_id(), Some(&it.test), None);
        self.looped(trips, |this| walk::walk_while_statement(this, it));
    }

    fn visit_do_while_statement(&mut self, it: &DoWhileStatement<'a>) {
        let trips = self.trips(it.node_id(), Some(&it.test), None);
        self.looped(trips, |this| walk::walk_do_while_statement(this, it));
    }

    fn visit_for_of_statement(&mut self, it: &ForOfStatement<'a>) {
        self.visit_expression(&it.right);
        let trips = self.model.trips;
        self.looped(trips, |this| {
            this.visit_for_statement_left(&it.left);
            this.visit_statement(&it.body);
        });
    }

    fn visit_for_in_statement(&mut self, it: &ForInStatement<'a>) {
        self.visit_expression(&it.right);
        let trips = self.model.trips;
        self.looped(trips, |this| {
            this.visit_for_statement_left(&it.left);
            this.visit_statement(&it.body);
        });
    }

    fn visit_binary_expression(&mut self, it: &BinaryExpression<'a>) {
        let bitwise = matches!(
            it.operator,
            BinaryOperator::BitwiseAnd
                | BinaryOperator::BitwiseOR
                | BinaryOperator::BitwiseXOR
                | BinaryOperator::ShiftLeft
                | BinaryOperator::ShiftRight
                | BinaryOperator::ShiftRightZeroFill
        );
        let integer = bitwise || (self.integer(&it.left) && self.integer(&it.right));
        let weight = if it.operator == BinaryOperator::Exponential {
            self.model.math
        } else {
            self.model.arithmetic
        };
        self.add(if integer {
            weight * self.model.integer
        } else {
            weight
        });
        walk::walk_binary_expression(self, it);
    }

    fn visit_unary_expression(&mut self, it: &UnaryExpression<'a>) {
        self.add(self.model.arithmetic);
        walk::walk_unary_expression(self, it);
    }

    fn visit_update_expression(&mut self, it: &UpdateExpression<'a>) {
        self.add(self.model.arithmetic * self.model.integer);
        walk::walk_update_expression(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if it.operator != AssignmentOperator::Assign {
            self.add(self.model.arithmetic);
        }
        match &it.left {
            AssignmentTarget::ComputedMemberExpression(member) => self.written(&member.object),
            AssignmentTarget::StaticMemberExpression(member) => self.written(&member.object),
            _ => {}
        }
        walk::walk_assignment_expression(self, it);
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        let weight = if self.typed_array(&it.object) {
            self.model.index * self.model.typed_array
        } else {
            self.model.index
        };
        self.add(weight);
        walk::walk_computed_member_expression(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let math = matches!(it.callee.get_inner_expression(),
            Expression::StaticMemberExpression(member)
                if matches!(&member.object, Expression::Identifier(id) if id.name == "Math"));
        if math {
            self.add(self.model.math);
        } else {
            self.site(it.node_id());
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        self.site(it.node_id());
        walk::walk_new_expression(self, it);
    }

    fn visit_return_statement(&mut self, it: &ReturnStatement<'a>) {
        if let Some(argument) = &it.argument {
            let ty = self
                .facts
                .ty(self.node(argument.get_inner_expression().node_id()));
            let marshal = marshal_of(ty.value);
            if marshal != Marshal::Scalar {
                self.frame().metrics.returns = Some(marshal);
            }
        }
        walk::walk_return_statement(self, it);
    }
}

/// `a op b` as `b op' a`.
fn flip(operator: BinaryOperator) -> Option<BinaryOperator> {
    Some(match operator {
        BinaryOperator::LessThan => BinaryOperator::GreaterThan,
        BinaryOperator::LessEqualThan => BinaryOperator::GreaterEqualThan,
        BinaryOperator::GreaterThan => BinaryOperator::LessThan,
        BinaryOperator::GreaterEqualThan => BinaryOperator::LessEqualThan,
        _ => return None,
    })
}
