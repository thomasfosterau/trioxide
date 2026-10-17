//! Converting tsrs types into trioxide's [`Type`] lattice (PLAN §3).
//!
//! The conversion is conservative: anything without a faithful trioxide equivalent becomes
//! [`Type::Unknown`]. In particular `any`, `unknown`, `void` (a `void` function may return
//! anything) and unconstrained type parameters are `Unknown`, and object types become shapes
//! only when they are plain property bags declared in the program itself.

use trioxide_common::NodeRef;
use tsrs_ast::{self as ast, Kind, Node, Symbol, SymbolFlags};
use tsrs_checker::{ObjectFlags, Signature as TsSignature, Type as TsType, TypeFlags};
use tsrs_compiler::Checker;
use tsrs_core::P;

use crate::ty::{NumberType, Shape, Signature, Type, TypedArrayKind};

/// How deep object, array and function types are converted before the rest is `Unknown`.
const MAX_DEPTH: usize = 4;
/// Object types with more properties than this are not converted into shapes.
const MAX_PROPERTIES: usize = 32;

/// Global symbols the conversion compares against, looked up once per checker.
pub struct Globals {
    typed_arrays: Vec<(P<Symbol>, TypedArrayKind)>,
}

impl Globals {
    pub fn new(checker: &mut Checker) -> Self {
        let typed_arrays = TypedArrayKind::ALL
            .into_iter()
            .filter_map(|kind| {
                checker
                    .get_global_symbol_exported(kind.constructor_name(), SymbolFlags::Type, None)
                    .map(|symbol| (symbol, kind))
            })
            .collect();
        Self { typed_arrays }
    }
}

/// Converts types of one checker.
pub struct Converter<'a> {
    pub checker: &'a mut Checker,
    pub globals: &'a Globals,
    /// Maps a class or function declaration to the oxc node it corresponds to, if registered.
    pub resolve: &'a dyn Fn(P<Node>) -> Option<NodeRef>,
}

impl Converter<'_> {
    /// Converts `ty`.
    pub fn convert(&mut self, ty: P<TsType>) -> Type {
        self.convert_at(ty, MAX_DEPTH)
    }

    fn convert_at(&mut self, ty: P<TsType>, depth: usize) -> Type {
        if depth == 0 {
            return Type::Unknown;
        }
        let flags = ty.flags();
        if flags.intersects(TypeFlags::Any | TypeFlags::Unknown | TypeFlags::Void) {
            return Type::Unknown;
        }
        if flags.intersects(TypeFlags::Never) {
            return Type::Never;
        }
        if flags.intersects(TypeFlags::Union) {
            let members: Vec<P<TsType>> = ty.types().to_vec();
            return Type::union(
                members
                    .into_iter()
                    .map(|member| self.convert_at(member, depth)),
            );
        }
        if flags.intersects(TypeFlags::NumberLiteral) {
            return match ty.as_literal_type().value() {
                Some(tsrs_checker::LiteralValue::Number(number)) => {
                    Type::Number(NumberType::constant(number.0))
                }
                _ => Type::NUMBER,
            };
        }
        if flags.intersects(TypeFlags::Number | TypeFlags::Enum) {
            return Type::NUMBER;
        }
        if flags.intersects(
            TypeFlags::String
                | TypeFlags::StringLiteral
                | TypeFlags::TemplateLiteral
                | TypeFlags::StringMapping,
        ) {
            return Type::String;
        }
        if flags.intersects(TypeFlags::Boolean | TypeFlags::BooleanLiteral) {
            return Type::Boolean;
        }
        if flags.intersects(TypeFlags::BigInt | TypeFlags::BigIntLiteral) {
            return Type::BigInt;
        }
        if flags.intersects(TypeFlags::ESSymbol | TypeFlags::UniqueESSymbol) {
            return Type::Symbol;
        }
        if flags.intersects(TypeFlags::Null) {
            return Type::Null;
        }
        if flags.intersects(TypeFlags::Undefined) {
            return Type::Undefined;
        }
        if flags.intersects(
            TypeFlags::TypeParameter
                | TypeFlags::IndexedAccess
                | TypeFlags::Conditional
                | TypeFlags::Substitution,
        ) {
            // A generic value is anything its constraint allows.
            return match self.checker.get_base_constraint_of_type_exported(ty) {
                Some(constraint) if constraint != ty => self.convert_at(constraint, depth - 1),
                _ => Type::Unknown,
            };
        }
        if flags.intersects(TypeFlags::Object) {
            return self.convert_object(ty, depth);
        }
        Type::Unknown
    }

    fn convert_object(&mut self, ty: P<TsType>, depth: usize) -> Type {
        if self.checker.is_array_type_exported(ty) {
            let element = self
                .checker
                .get_type_arguments(ty)
                .first()
                .copied()
                .map_or(Type::Unknown, |element| self.convert_at(element, depth - 1));
            return Type::Array(Box::new(element));
        }
        if tsrs_checker::is_tuple_type_exported(ty) {
            return self.convert_tuple(ty, depth);
        }
        let symbol = ty.symbol();
        if let Some(symbol) = symbol
            && let Some(&(_, kind)) = self
                .globals
                .typed_arrays
                .iter()
                .find(|(global, _)| *global == symbol)
        {
            return Type::TypedArray(kind);
        }
        let object_flags = ty.object_flags();
        let class_instance = object_flags.intersects(ObjectFlags::Class)
            || (object_flags.intersects(ObjectFlags::Reference)
                && ty
                    .target()
                    .is_some_and(|target| target.object_flags().intersects(ObjectFlags::Class)));
        if class_instance {
            return symbol
                .and_then(class_declaration)
                .and_then(|declaration| (self.resolve)(declaration))
                .map_or(Type::Unknown, Type::Instance);
        }

        let calls = self.checker.get_call_signatures(ty);
        let constructs = self.checker.get_construct_signatures(ty);
        if !constructs.is_empty() {
            return Type::Unknown;
        }
        if !calls.is_empty() {
            let id = symbol
                .and_then(function_declaration)
                .and_then(|declaration| (self.resolve)(declaration));
            return match calls {
                [signature] => {
                    let signature = self.convert_signature_at(*signature, depth - 1);
                    Type::Function(Box::new(signature), id)
                }
                // An overloaded function: callable, but which overload applies depends on the call.
                _ => Type::Function(Box::default(), id),
            };
        }

        if !self.is_plain_object(ty, symbol) {
            return Type::Unknown;
        }
        let properties = self.checker.get_properties_of_type_exported(ty);
        if properties.len() > MAX_PROPERTIES {
            return Type::Unknown;
        }
        let mut shape = Shape::default();
        for &property in properties {
            if !property.flags().intersects(SymbolFlags::Property) {
                return Type::Unknown;
            }
            let property_type = self.checker.get_type_of_symbol_exported(property);
            let mut converted = self.convert_at(property_type, depth - 1);
            if property.flags().intersects(SymbolFlags::Optional) {
                converted = Type::union([converted, Type::Undefined]);
            }
            shape
                .properties
                .push((property.name().to_string(), converted));
        }
        Type::Object(shape)
    }

    /// Whether `ty` is a plain property bag: no index signatures and declared (if at all) in
    /// the program's own sources, not in a library's declarations.
    fn is_plain_object(&mut self, ty: P<TsType>, symbol: Option<P<Symbol>>) -> bool {
        if !self.checker.get_index_infos_of_type_exported(ty).is_empty() {
            return false;
        }
        let Some(symbol) = symbol else {
            return true;
        };
        symbol.declarations().iter().all(|&declaration| {
            ast::get_source_file_of_node(declaration)
                .is_some_and(|file| !file.is_declaration_file())
        })
    }

    /// Converts a resolved signature.
    pub fn convert_signature(&mut self, signature: P<TsSignature>) -> Signature {
        self.convert_signature_at(signature, MAX_DEPTH)
    }

    fn convert_signature_at(&mut self, signature: P<TsSignature>, depth: usize) -> Signature {
        if depth == 0 {
            return Signature::default();
        }
        let params = signature
            .parameters()
            .iter()
            .map(|&param| {
                let ty = self.checker.get_type_of_symbol_exported(param);
                self.convert_at(ty, depth)
            })
            .collect();
        let ret = self
            .checker
            .get_return_type_of_signature_exported(signature);
        Signature {
            params,
            rest: signature.has_rest_parameter(),
            ret: self.convert_at(ret, depth),
        }
    }

    fn convert_tuple(&mut self, ty: P<TsType>, depth: usize) -> Type {
        let Some(target) = ty.target() else {
            return Type::Unknown;
        };
        let tuple = target.as_tuple_type();
        let arguments: Vec<P<TsType>> = self.checker.get_type_arguments(ty).to_vec();
        let fixed = tuple.element_infos().len() == tuple.fixed_length()
            && tuple
                .element_infos()
                .iter()
                .all(|info| info.flags == tsrs_checker::ElementFlags::Required);
        let elements: Vec<Type> = arguments
            .iter()
            .take(tuple.element_infos().len())
            .map(|&element| self.convert_at(element, depth - 1))
            .collect();
        if fixed {
            Type::Tuple(elements)
        } else {
            // Optional or rest elements: still an array, of any of the element types.
            Type::Array(Box::new(Type::union(
                elements.into_iter().chain([Type::Undefined]),
            )))
        }
    }
}

fn class_declaration(symbol: P<Symbol>) -> Option<P<Node>> {
    if !symbol.flags().intersects(SymbolFlags::Class) {
        return None;
    }
    let declarations = symbol.declarations();
    match declarations {
        [declaration]
            if matches!(
                declaration.kind(),
                Kind::ClassDeclaration | Kind::ClassExpression
            ) =>
        {
            Some(*declaration)
        }
        // Merged with an interface or namespace: the class's shape is not its own.
        _ => None,
    }
}

fn function_declaration(symbol: P<Symbol>) -> Option<P<Node>> {
    let declaration = symbol.value_declaration()?;
    ast::is_function_like(declaration).then_some(declaration)
}
