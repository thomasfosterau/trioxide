//! Matching oxc nodes to tsrs nodes by UTF-8 byte span (PLAN §3 "Span correlation").
//!
//! tsrs nodes start at their leading trivia (`pos`), oxc nodes at their first token, so every
//! tsrs node is indexed under `(token start, end)`. Several tsrs nodes can share a span (an
//! identifier and the expression statement around it); each is ranked by how directly it
//! denotes a value, and the best one wins. Nodes the parser synthesised (`Reparsed`) and nodes
//! inside `JSDoc` are not indexed: they have no oxc counterpart.
//!
//! oxc wraps exported declarations in an `export` node while tsrs makes `export` a modifier of
//! the declaration, so declarations whose modifiers start with `export` / `default` are also
//! indexed from the first token after them.

use rustc_hash::FxHashMap;
use tsrs_ast::{self as ast, Kind, Node, NodeFlags, SourceFile};
use tsrs_core::P;

/// How directly a tsrs node denotes a value; lower wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    Name,
    Expression,
    Declaration,
    Statement,
    Other,
    /// Only reachable through the `export`-stripped start.
    Stripped,
}

/// The tsrs nodes of one source file, by `(token start, end)`.
pub struct SpanMap {
    nodes: FxHashMap<(u32, u32), (Rank, P<Node>)>,
}

impl SpanMap {
    /// Indexes every node of `file`.
    pub fn build(file: P<SourceFile>) -> Self {
        let mut map = Self {
            nodes: FxHashMap::default(),
        };
        let text = file.text();
        file.as_node().for_each_child(&mut |child| {
            map.visit(child, file, text);
            false
        });
        map
    }

    fn visit(&mut self, node: P<Node>, file: P<SourceFile>, text: &str) {
        let flags = node.flags();
        if flags.intersects(NodeFlags::JSDoc) {
            return;
        }
        if !flags.intersects(NodeFlags::Reparsed)
            && let Some(end) = node.end().source_offset()
        {
            let start = tsrs_astnav::get_start_of_node(node, file, false);
            if start < end {
                let rank = rank(node);
                self.insert(start, end, rank, node);
                if let Some(stripped) = start_after_export(node, text) {
                    self.insert(stripped, end, Rank::Stripped, node);
                }
            }
        }
        node.for_each_child(&mut |child| {
            self.visit(child, file, text);
            false
        });
    }

    fn insert(&mut self, start: u32, end: u32, rank: Rank, node: P<Node>) {
        // Children are visited after parents, so on a tie the innermost node wins.
        match self.nodes.get(&(start, end)) {
            Some((best, _)) if *best < rank => {}
            _ => {
                self.nodes.insert((start, end), (rank, node));
            }
        }
    }

    /// The tsrs node whose first token starts at `start` and which ends at `end`.
    pub fn get(&self, start: u32, end: u32) -> Option<P<Node>> {
        self.nodes.get(&(start, end)).map(|&(_, node)| node)
    }
}

fn rank(node: P<Node>) -> Rank {
    match node.kind() {
        Kind::Identifier
        | Kind::PrivateIdentifier
        | Kind::ThisKeyword
        | Kind::SuperKeyword
        | Kind::NullKeyword
        | Kind::TrueKeyword
        | Kind::FalseKeyword
        | Kind::NumericLiteral
        | Kind::BigIntLiteral
        | Kind::StringLiteral
        | Kind::NoSubstitutionTemplateLiteral
        | Kind::RegularExpressionLiteral => Rank::Name,
        _ if ast::is_expression_node(node) => Rank::Expression,
        _ if ast::is_declaration(node) || node.kind() == Kind::Parameter => Rank::Declaration,
        _ if ast::is_statement(node) => Rank::Statement,
        _ => Rank::Other,
    }
}

/// For a declaration whose modifiers start with `export` (and `default`), the first token after
/// them, where oxc starts the declaration.
fn start_after_export(node: P<Node>, text: &str) -> Option<u32> {
    let modifiers = node.modifier_nodes();
    let mut last = None;
    for modifier in modifiers {
        match modifier.kind() {
            Kind::ExportKeyword | Kind::DefaultKeyword => last = Some(*modifier),
            _ => break,
        }
    }
    let end = last?.end().source_offset()?;
    Some(skip_trivia(text, end))
}

/// Skips whitespace and comments from byte `pos`.
fn skip_trivia(text: &str, mut pos: u32) -> u32 {
    let bytes = text.as_bytes();
    let at = |pos: u32| bytes.get(pos as usize).copied();
    loop {
        match (at(pos), at(pos + 1)) {
            (Some(b' ' | b'\t' | b'\n' | b'\r'), _) => pos += 1,
            (Some(b'/'), Some(b'/')) => {
                while at(pos).is_some_and(|byte| byte != b'\n') {
                    pos += 1;
                }
            }
            (Some(b'/'), Some(b'*')) => {
                pos += 2;
                while at(pos).is_some() && !(at(pos) == Some(b'*') && at(pos + 1) == Some(b'/')) {
                    pos += 1;
                }
                pos += 2;
            }
            _ => return pos,
        }
    }
}
