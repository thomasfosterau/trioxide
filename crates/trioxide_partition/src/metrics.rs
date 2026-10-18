//! Static estimates the cost model scores with: per function, its compute weight per call and
//! the calls it makes; per call site, how often it runs per run of its function.
//!
//! Modules are parsed the way the analysis parses them, so node ids match the [`Facts`].

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrowFunctionExpression, AssignmentExpression, AssignmentOperator, AssignmentTarget,
    BinaryExpression, CallExpression, ComputedMemberExpression, DoWhileStatement, Expression,
    ForInStatement, ForOfStatement, ForStatement, ForStatementInit, FormalParameters, Function,
    NewExpression, ReturnStatement, StaticMemberExpression, UnaryExpression, UpdateExpression,
    WhileStatement,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_semantic::{Scoping, SemanticBuilder};
use oxc_syntax::{operator::BinaryOperator, scope::ScopeFlags};
use rustc_hash::{FxHashMap, FxHashSet};
use trioxide_analysis::{Facts, Trust};
use trioxide_common::{ModuleId, NodeRef, SymbolRef};
use trioxide_types::{FunctionId, NumberType, Type, TypedArrayKind};

use crate::{CostModel, SourceModule};

/// The largest count the estimates use: deep loop nests saturate here instead of
/// overflowing to infinity (and `∞ × 0` to NaN).
const SATURATION: f64 = 1e18;

/// `value`, at most [`SATURATION`].
pub(crate) fn saturate(value: f64) -> f64 {
    value.min(SATURATION)
}

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

/// Operation counts by kind, each weighted separately by the cost model.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Ops {
    /// Floating-point arithmetic and comparisons.
    pub arithmetic: f64,
    /// Arithmetic, comparisons, `++`/`--` and bitwise operators on values the island holds as
    /// int32s.
    pub integer: f64,
    /// Float operands of bitwise operators and `Math.imul`/`Math.clz32`: a `ToInt32` each.
    pub conversion: f64,
    /// `(a + b) | 0`-style arithmetic: int32 operations whose result may leave the int32
    /// range and is wrapped back.
    pub wrapping: f64,
    /// `Math.*` calls and `**`.
    pub math: f64,
    /// `%` on values not proven integers: wasm32 has no float remainder instruction, so it is
    /// a call to `fmod`.
    pub remainder: f64,
    /// Array element reads and writes whose index the island computes as an int32.
    pub index: f64,
    /// Other element reads and writes: the index is a float, converted and checked per
    /// access.
    pub checked_index: f64,
    /// Loop iterations.
    pub iteration: f64,
}

impl Ops {
    /// `self + other × factor`.
    pub fn add_scaled(&mut self, other: &Self, factor: f64) {
        let add =
            |count: &mut f64, other: f64| *count = saturate(*count + saturate(other * factor));
        add(&mut self.arithmetic, other.arithmetic);
        add(&mut self.integer, other.integer);
        add(&mut self.conversion, other.conversion);
        add(&mut self.wrapping, other.wrapping);
        add(&mut self.math, other.math);
        add(&mut self.remainder, other.remainder);
        add(&mut self.index, other.index);
        add(&mut self.checked_index, other.checked_index);
        add(&mut self.iteration, other.iteration);
    }

    /// Every operation counted once.
    pub fn total(&self) -> f64 {
        self.arithmetic
            + self.integer
            + self.conversion
            + self.wrapping
            + self.math
            + self.remainder
            + self.index
            + self.checked_index
            + self.iteration
    }

    /// The time WASM saves running them, by `model`'s weights.
    pub fn saving(&self, model: &CostModel) -> f64 {
        self.arithmetic * model.arithmetic
            + self.integer * model.integer
            + self.conversion * model.conversion
            + self.wrapping * model.wrapping
            + self.math * model.math
            + self.remainder * model.remainder
            + self.index * model.index
            + self.checked_index * model.checked_index
            + self.iteration * model.iteration
    }
}

/// One kind of operation.
#[derive(Debug, Clone, Copy)]
enum Op {
    Arithmetic,
    Integer,
    Conversion,
    Wrapping,
    Math,
    Remainder,
    Index,
    CheckedIndex,
    Iteration,
}

/// One function's estimates.
#[derive(Debug, Clone, Default)]
pub(crate) struct FunctionMetrics {
    /// Operations per call, loops multiplied in; calls excluded.
    pub ops: Ops,
    /// Call sites in its body (not in nested functions), with how often each runs per call.
    pub calls: Vec<(NodeRef, f64)>,
    /// Its parameters, with whether its body writes their elements or fields, and the
    /// largest `length` the analysis knows for them.
    pub params: Vec<Param>,
    /// Arrays it returns, if any.
    pub returns: Option<Marshal>,
}

/// One parameter of a function.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Param {
    pub symbol: SymbolRef,
    /// The body writes its elements or fields.
    pub written: bool,
    /// The largest `length` the analysis knows for it, from the body's `p.length` reads.
    pub length: Option<f64>,
}

/// Estimates for a whole program.
#[derive(Debug, Default)]
pub(crate) struct Metrics {
    pub functions: FxHashMap<FunctionId, FunctionMetrics>,
    /// How often each call site runs per run of the unit containing it.
    pub sites: FxHashMap<NodeRef, f64>,
    /// Call sites whose callee is a plain identifier: the calls the facade can point at an
    /// island's glue.
    pub direct: FxHashSet<NodeRef>,
}

impl Metrics {
    /// Walks every module.
    pub fn measure(modules: &[SourceModule<'_>], facts: &Facts, model: &CostModel) -> Self {
        let allocators: Vec<Allocator> = modules.iter().map(|_| Allocator::default()).collect();
        let parsed: Vec<_> = modules
            .iter()
            .zip(&allocators)
            .map(|(module, allocator)| {
                let parsed = Parser::new(allocator, module.source, module.source_type).parse();
                let program = allocator.alloc(parsed.program);
                let semantic = SemanticBuilder::new()
                    .with_enum_eval(true)
                    .with_build_nodes(true)
                    .build(program)
                    .semantic;
                (module.id, &*program, semantic)
            })
            .collect();
        let mut lengths = Lengths::default();
        for (module, program, semantic) in &parsed {
            let mut collector = Collector {
                module: *module,
                facts,
                scoping: semantic.scoping(),
                out: &mut lengths,
            };
            collector.visit_program(program);
        }
        let hints = lengths.hints(facts);
        let mut metrics = Self::default();
        for (module, program, semantic) in &parsed {
            let mut walker = Walker {
                module: *module,
                facts,
                model,
                scoping: semantic.scoping(),
                hints: &hints,
                lengths: &lengths,
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

    /// The arrays and objects passed to or returned from one call of `function`.
    pub fn aggregates(&self, function: FunctionId, facts: &Facts) -> f64 {
        let Some(metrics) = self.functions.get(&function) else {
            return 0.0;
        };
        let params = metrics
            .params
            .iter()
            .filter(|param| marshal_of(facts.symbol_ty(param.symbol).value) != Marshal::Scalar)
            .count();
        #[expect(clippy::cast_precision_loss, reason = "parameter counts are small")]
        let params = params as f64;
        params + if metrics.returns.is_some() { 1.0 } else { 0.0 }
    }

    /// The bytes copied across the boundary per call of `function`.
    pub fn bytes(&self, function: FunctionId, facts: &Facts, model: &CostModel) -> f64 {
        let Some(metrics) = self.functions.get(&function) else {
            return 0.0;
        };
        let size = |marshal: Marshal, written: bool, length: Option<f64>| {
            let copies = if written { 2.0 } else { 1.0 };
            let elements = length.unwrap_or(model.trips);
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
            .map(|param| {
                size(
                    marshal_of(facts.symbol_ty(param.symbol).value),
                    param.written,
                    param.length,
                )
            })
            .sum();
        params
            + metrics
                .returns
                .map_or(0.0, |marshal| size(marshal, false, None))
    }
}

/// Whether every value of `n` is an int32 other than `-0` (as the Rust backend requires).
fn is_int32(n: NumberType) -> bool {
    n.int
        && n.non_nan
        && n.non_neg_zero
        && n.range.min >= f64::from(i32::MIN)
        && n.range.max <= f64::from(i32::MAX)
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

/// Array lengths the program's allocation sites give, and what reaches each parameter.
#[derive(Default)]
struct Lengths {
    /// Typed arrays of known length, by allocation site.
    allocations: FxHashMap<NodeRef, f64>,
    /// Array literals and their lengths when made (an estimate: `push` may grow them).
    literals: FxHashMap<NodeRef, f64>,
    /// Each call site's arguments.
    arguments: FxHashMap<NodeRef, Vec<NodeRef>>,
    /// Each function's parameters.
    params: FxHashMap<FunctionId, Vec<SymbolRef>>,
}

impl Lengths {
    /// The longest array that may reach each parameter, when every array that may is
    /// allocated with a known length.
    fn hints(&self, facts: &Facts) -> FxHashMap<SymbolRef, f64> {
        let mut hints: FxHashMap<SymbolRef, Option<f64>> = FxHashMap::default();
        for edge in facts.call_graph().edges() {
            let (Some(arguments), Some(params)) = (
                self.arguments.get(&edge.site),
                self.params.get(&edge.callee),
            ) else {
                continue;
            };
            for (param, argument) in params.iter().zip(arguments) {
                let length = facts.points_to(*argument).value.and_then(|sites| {
                    sites
                        .iter()
                        .map(|site| self.allocations.get(site).copied())
                        .collect::<Option<Vec<f64>>>()?
                        .into_iter()
                        .reduce(f64::max)
                });
                let hint = hints.entry(*param).or_insert(length);
                *hint = match (*hint, length) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    _ => None,
                };
            }
        }
        hints
            .into_iter()
            .filter_map(|(param, hint)| Some((param, hint?)))
            .collect()
    }
}

/// Fills [`Lengths`] from one module.
struct Collector<'w, 's> {
    module: ModuleId,
    facts: &'w Facts,
    scoping: &'s Scoping,
    out: &'w mut Lengths,
}

impl Collector<'_, '_> {
    fn node(&self, node: oxc_syntax::node::NodeId) -> NodeRef {
        NodeRef {
            module: self.module,
            node,
        }
    }

    fn arguments(&mut self, site: oxc_syntax::node::NodeId, arguments: &[Argument<'_>]) {
        let arguments = arguments
            .iter()
            .map(|argument| match argument.as_expression() {
                Some(expression) => self.node(expression.get_inner_expression().node_id()),
                None => self.node(argument.node_id()),
            })
            .collect();
        self.out.arguments.insert(self.node(site), arguments);
    }

    fn params(&mut self, id: oxc_syntax::node::NodeId, params: &FormalParameters<'_>) {
        let params = params
            .items
            .iter()
            .filter_map(|param| param.pattern.get_binding_identifier()?.symbol_id.get())
            .map(|symbol| SymbolRef {
                module: self.module,
                symbol,
            })
            .collect();
        self.out.params.insert(self.node(id), params);
    }
}

impl<'a> Visit<'a> for Collector<'_, '_> {
    fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
        self.params(function.node_id(), &function.params);
        walk::walk_function(self, function, flags);
    }

    fn visit_arrow_function_expression(&mut self, arrow: &ArrowFunctionExpression<'a>) {
        self.params(arrow.node_id(), &arrow.params);
        walk::walk_arrow_function_expression(self, arrow);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        self.arguments(it.node_id(), &it.arguments);
        walk::walk_call_expression(self, it);
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        self.arguments(it.node_id(), &it.arguments);
        // Typed arrays keep their length (number[] may grow, so their literals say nothing).
        let typed = matches!(&it.callee, Expression::Identifier(id)
            if id.name.ends_with("Array") && id.name != "Array"
                && id.reference_id.get().is_some_and(|r| self.scoping.get_reference(r).symbol_id().is_none()));
        if typed && let [argument] = it.arguments.as_slice() {
            let length = match argument
                .as_expression()
                .map(Expression::get_inner_expression)
            {
                #[expect(clippy::cast_precision_loss, reason = "literal lengths are small")]
                Some(Expression::ArrayExpression(elements))
                    if elements.elements.iter().all(|e| !e.is_spread()) =>
                {
                    Some(elements.elements.len() as f64)
                }
                Some(expression) => {
                    let fact = self.facts.range(self.node(expression.node_id()));
                    let range = fact.value.range;
                    (fact.trust > Trust::Unknown && range.max < f64::from(1 << 24))
                        .then_some(range.max)
                }
                None => None,
            };
            if let Some(length) = length {
                self.out.allocations.insert(self.node(it.node_id()), length);
            }
        }
        walk::walk_new_expression(self, it);
    }

    fn visit_array_expression(&mut self, it: &oxc_ast::ast::ArrayExpression<'a>) {
        if !it.elements.is_empty() && it.elements.iter().all(|e| !e.is_spread()) {
            #[expect(clippy::cast_precision_loss, reason = "literal lengths are small")]
            let length = it.elements.len() as f64;
            self.out.literals.insert(self.node(it.node_id()), length);
        }
        walk::walk_array_expression(self, it);
    }
}

/// Whether every local an expression reads is held as an int32 by the backend.
struct Int32Locals<'w, 's, 'x> {
    walker: &'x Walker<'w, 's>,
    int32: bool,
}

impl<'a> Visit<'a> for Int32Locals<'_, '_, '_> {
    fn visit_identifier_reference(&mut self, id: &oxc_ast::ast::IdentifierReference<'a>) {
        let symbol = id
            .reference_id
            .get()
            .and_then(|r| self.walker.scoping.get_reference(r).symbol_id());
        // Globals (`Math`) aren't locals the backend types.
        if let Some(symbol) = symbol {
            self.int32 &= self.walker.int32_symbol(symbol);
        }
    }
}

struct Walker<'w, 's> {
    module: ModuleId,
    facts: &'w Facts,
    model: &'w CostModel,
    scoping: &'s Scoping,
    /// Array lengths that reach parameters.
    hints: &'w FxHashMap<SymbolRef, f64>,
    lengths: &'w Lengths,
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

    fn add(&mut self, op: Op) {
        let frame = self.frame();
        let ops = &mut frame.metrics.ops;
        let count = match op {
            Op::Arithmetic => &mut ops.arithmetic,
            Op::Integer => &mut ops.integer,
            Op::Conversion => &mut ops.conversion,
            Op::Wrapping => &mut ops.wrapping,
            Op::Math => &mut ops.math,
            Op::Remainder => &mut ops.remainder,
            Op::Index => &mut ops.index,
            Op::CheckedIndex => &mut ops.checked_index,
            Op::Iteration => &mut ops.iteration,
        };
        *count = saturate(*count + frame.freq);
    }

    /// Whether the Rust backend computes `expression` as an int32: it is a numeric literal
    /// (folded at compile time), or the analysis proves the value is one, and so every local
    /// it reads (the backend types a local by its binding's range, not by the range at each
    /// use). `+`, `-` and `*` run on int32s only when both operands do, so their operands
    /// must qualify too (`(y * width + x) * 4` is float arithmetic when `y * width` has no
    /// int32 range, whatever the sum's range).
    fn int32(&self, expression: &Expression<'_>) -> bool {
        let inner = expression.get_inner_expression();
        if matches!(inner, Expression::NumericLiteral(_)) {
            return true;
        }
        let fact = self.facts.range(self.node(inner.node_id()));
        if !(self.facts.is_usable(fact.trust) && is_int32(fact.value)) {
            return false;
        }
        if let Expression::BinaryExpression(binary) = inner
            && matches!(
                binary.operator,
                BinaryOperator::Addition
                    | BinaryOperator::Subtraction
                    | BinaryOperator::Multiplication
            )
        {
            return self.int32(&binary.left) && self.int32(&binary.right);
        }
        let mut locals = Int32Locals {
            walker: self,
            int32: true,
        };
        locals.visit_expression(expression);
        locals.int32
    }

    /// Whether the backend holds the local `symbol` as an int32 (its range fact proves one).
    fn int32_symbol(&self, symbol: oxc_syntax::symbol::SymbolId) -> bool {
        let fact = self.facts.symbol_ty(SymbolRef {
            module: self.module,
            symbol,
        });
        self.facts.is_usable(fact.trust) && matches!(fact.value, Type::Number(n) if is_int32(*n))
    }

    /// Whether the analysis knows the value of `expression` is an integer.
    fn integer(&self, expression: &Expression<'_>) -> bool {
        let fact = self.facts.range(self.node(expression.node_id()));
        fact.trust > Trust::Unknown && fact.value.int
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

    /// `expression` as a counter plus a constant offset (`i`, `i + 3`, `i - 1`).
    fn counter(&self, expression: &Expression<'_>) -> Option<(SymbolRef, f64)> {
        if let Some(symbol) = self.symbol(expression) {
            return Some((symbol, 0.0));
        }
        let Expression::BinaryExpression(sum) = expression.get_inner_expression() else {
            return None;
        };
        let Expression::NumericLiteral(constant) = sum.right.get_inner_expression() else {
            return None;
        };
        let symbol = self.symbol(&sum.left)?;
        match sum.operator {
            BinaryOperator::Addition => Some((symbol, constant.value)),
            BinaryOperator::Subtraction => Some((symbol, -constant.value)),
            _ => None,
        }
    }

    /// A finite bound the analysis knows for `expression`: its range's `max` (or `min`).
    fn bound(&self, expression: &Expression<'_>, max: bool) -> Option<f64> {
        if let Expression::StaticMemberExpression(member) = expression.get_inner_expression()
            && member.property.name == "length"
            && let Some(&hint) = self
                .symbol(&member.object)
                .and_then(|symbol| self.hints.get(&symbol))
        {
            return Some(hint);
        }
        self.bound_of(expression.get_inner_expression().node_id(), max)
    }

    /// [`bound`](Self::bound) of the expression `node`.
    fn bound_of(&self, node: oxc_syntax::node::NodeId, max: bool) -> Option<f64> {
        let fact = self.facts.range(self.node(node));
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
            let ((counter, offset), operator, limit) =
                match (self.counter(&test.left), self.counter(&test.right)) {
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
            let distance = (to - offset - from) / step;
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
        self.frame().freq = saturate(outer * trips);
        self.add(Op::Iteration);
        walk(self);
        self.frame().freq = outer;
    }

    fn enter(&mut self, id: FunctionId, params: Vec<SymbolRef>) {
        self.frames.push(Frame {
            id: Some(id),
            freq: 1.0,
            metrics: FunctionMetrics {
                params: params
                    .into_iter()
                    .map(|symbol| Param {
                        symbol,
                        written: false,
                        length: self.hints.get(&symbol).copied(),
                    })
                    .collect(),
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

    /// The parameter of the current function `expression` names, if it names one.
    fn param(&mut self, expression: &Expression<'_>) -> Option<&mut Param> {
        let symbol = self.symbol(expression)?;
        self.frames
            .last_mut()
            .expect("the program frame")
            .metrics
            .params
            .iter_mut()
            .find(|p| p.symbol == symbol)
    }

    fn params(&self, function: &FormalParameters<'_>) -> Vec<SymbolRef> {
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
        // The length of the arrays it may iterate, when all are literals or typed arrays of
        // known length.
        let right = self.node(it.right.get_inner_expression().node_id());
        let trips = self
            .facts
            .points_to(right)
            .value
            .and_then(|sites| {
                sites
                    .iter()
                    .map(|site| {
                        let lengths = self.lengths;
                        lengths
                            .literals
                            .get(site)
                            .or_else(|| lengths.allocations.get(site))
                            .copied()
                    })
                    .collect::<Option<Vec<f64>>>()?
                    .into_iter()
                    .reduce(f64::max)
            })
            .unwrap_or(self.model.trips);
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
        if bitwise {
            self.add(Op::Integer);
            // `(a + b) | 0`, `(a * b) >>> 0`: int32 arithmetic wrapped back into 32 bits
            // (no float conversion when its operands are int32s).
            let wraps = matches!(
                it.operator,
                BinaryOperator::BitwiseOR | BinaryOperator::ShiftRightZeroFill
            ) && matches!(it.right.get_inner_expression(),
                Expression::NumericLiteral(zero) if zero.value == 0.0)
                && matches!(it.left.get_inner_expression(), Expression::BinaryExpression(inner)
                    if matches!(inner.operator, BinaryOperator::Addition
                        | BinaryOperator::Subtraction | BinaryOperator::Multiplication)
                        && self.int32(&inner.left) && self.int32(&inner.right));
            if wraps {
                self.add(Op::Wrapping);
            } else if !self.int32(&it.left) {
                self.add(Op::Conversion);
            }
            if !self.int32(&it.right) {
                self.add(Op::Conversion);
            }
        } else if it.operator == BinaryOperator::Exponential {
            self.add(Op::Math);
        } else if it.operator == BinaryOperator::Remainder
            && !(self.integer(&it.left) && self.integer(&it.right))
        {
            self.add(Op::Remainder);
        } else if self.int32(&it.left) && self.int32(&it.right) {
            self.add(Op::Integer);
        } else {
            self.add(Op::Arithmetic);
        }
        walk::walk_binary_expression(self, it);
    }

    fn visit_unary_expression(&mut self, it: &UnaryExpression<'a>) {
        self.add(Op::Arithmetic);
        walk::walk_unary_expression(self, it);
    }

    fn visit_update_expression(&mut self, it: &UpdateExpression<'a>) {
        let int32 = match &it.argument {
            oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => id
                .reference_id
                .get()
                .and_then(|r| self.scoping.get_reference(r).symbol_id())
                .is_some_and(|symbol| self.int32_symbol(symbol)),
            _ => false,
        };
        self.add(if int32 { Op::Integer } else { Op::Arithmetic });
        walk::walk_update_expression(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if it.operator != AssignmentOperator::Assign {
            let target = match &it.left {
                AssignmentTarget::AssignmentTargetIdentifier(id) => id
                    .reference_id
                    .get()
                    .and_then(|r| self.scoping.get_reference(r).symbol_id())
                    .is_some_and(|symbol| self.int32_symbol(symbol)),
                _ => false,
            };
            if it.operator.is_bitwise() && !target {
                self.add(Op::Conversion);
            }
            self.add(
                if it.operator.is_bitwise() || target && self.int32(&it.right) {
                    Op::Integer
                } else {
                    Op::Arithmetic
                },
            );
        }
        let object = match &it.left {
            AssignmentTarget::ComputedMemberExpression(member) => Some(&member.object),
            AssignmentTarget::StaticMemberExpression(member) => Some(&member.object),
            _ => None,
        };
        if let Some(param) = object.and_then(|object| self.param(object)) {
            param.written = true;
        }
        walk::walk_assignment_expression(self, it);
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        self.add(if self.int32(&it.expression) {
            Op::Index
        } else {
            Op::CheckedIndex
        });
        walk::walk_computed_member_expression(self, it);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if it.property.name == "length"
            && let Some(length) = self.bound_of(it.node_id(), true)
            && let Some(param) = self.param(&it.object)
        {
            param.length = Some(param.length.map_or(length, |known| known.min(length)));
        }
        walk::walk_static_member_expression(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let math = matches!(it.callee.get_inner_expression(),
            Expression::StaticMemberExpression(member)
                if matches!(&member.object, Expression::Identifier(id) if id.name == "Math"));
        if math {
            self.add(Op::Math);
            let converts = matches!(it.callee.get_inner_expression(),
                Expression::StaticMemberExpression(member)
                    if matches!(member.property.name.as_str(), "imul" | "clz32"));
            if converts {
                for argument in &it.arguments {
                    if let Some(argument) = argument.as_expression()
                        && !self.int32(argument)
                    {
                        self.add(Op::Conversion);
                    }
                }
            }
        } else {
            self.site(it.node_id());
            if !it.optional && matches!(it.callee, Expression::Identifier(_)) {
                let site = self.node(it.node_id());
                self.out.direct.insert(site);
            }
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
