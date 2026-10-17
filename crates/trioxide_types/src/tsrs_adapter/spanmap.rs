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

/// Finds the tsrs nodes of one source file by `(token start, end)`.
///
/// A node whose token span is `[start, end)` contains that span, so it lies on the path from
/// the file down through the children containing the span; the search walks that path rather
/// than indexing every node (a file is queried far less often than it has nodes).
pub struct SpanMap {
    file: P<SourceFile>,
}

impl SpanMap {
    /// A finder for nodes of `file`.
    pub fn build(file: P<SourceFile>) -> Self {
        Self { file }
    }

    /// The tsrs node whose first token starts at `start` and which ends at `end`.
    pub fn get(&self, start: u32, end: u32) -> Option<P<Node>> {
        let file = self.file;
        let text = file.text();
        let mut best: Option<(Rank, P<Node>)> = None;
        let mut current = file.as_node();
        loop {
            let mut next = None;
            current.for_each_child(&mut |child| {
                let (Some(pos), Some(child_end)) =
                    (child.pos().source_offset(), child.end().source_offset())
                else {
                    return false;
                };
                if pos <= start && end <= child_end && !child.flags().intersects(NodeFlags::JSDoc) {
                    next = Some(child);
                    return true;
                }
                false
            });
            let Some(node) = next else { break };
            if !node.flags().intersects(NodeFlags::Reparsed)
                && let Some(node_end) = node.end().source_offset()
                && node_end == end
            {
                let candidates = [
                    (token_start(node, file, text) == start).then(|| rank(node)),
                    (start_after_export(node, text) == Some(start)).then_some(Rank::Stripped),
                ];
                for rank in candidates.into_iter().flatten() {
                    // Deeper nodes come later, so on a tie the innermost node wins.
                    if best.is_none_or(|(old, _)| rank <= old) {
                        best = Some((rank, node));
                    }
                }
            }
            current = node;
        }
        best.map(|(_, node)| node)
    }
}

/// Where `node`'s first token starts: tsrs's `get_start_of_node`, without its scanner when
/// only ASCII whitespace and comments lead.
fn token_start(node: P<Node>, file: P<SourceFile>, text: &str) -> u32 {
    node.pos()
        .source_offset()
        .filter(|_| node.kind() != Kind::JsxText)
        .and_then(|pos| ascii_start(text, pos))
        .unwrap_or_else(|| tsrs_astnav::get_start_of_node(node, file, false))
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

/// The first token at or after `pos`, if only ASCII whitespace and comments come before it
/// (what tsrs's `skip_trivia` would find, without running its scanner). `None` for anything
/// it would treat specially: other whitespace, a shebang, conflict markers.
fn ascii_start(text: &str, pos: u32) -> Option<u32> {
    let start = skip_trivia(text, pos);
    let bytes = text.as_bytes();
    let first = *bytes.get(start as usize)?;
    let special = !first.is_ascii()
        || (first == b'#' && start == 0)
        || matches!(first, b'<' | b'=' | b'>' | b'|');
    let clean = bytes
        .get(pos as usize..start as usize)?
        .iter()
        .all(u8::is_ascii);
    (!special && clean).then_some(start)
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
