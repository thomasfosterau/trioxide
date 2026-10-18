//! `trioxide explain`: the decision log as text (or JSON with `--json`).

use std::{fmt::Write as _, path::Path};

use trioxide::{
    Report, Span,
    log::{CostEstimate, FunctionDecision, ModuleLog, PassOutcome, Target},
};

/// Renders `report` for humans: per module, each function's verdict, why, the failed
/// eligibility rules, the cost-model numbers and what the JS passes did in it. Paths are
/// shown relative to `root`; `source` gives a module's text, for line and column numbers.
pub fn render(report: &Report, root: &Path, source: &dyn Fn(&Path) -> Option<String>) -> String {
    let mut out = String::new();
    for module in &report.modules {
        if module.log.functions.is_empty() {
            continue;
        }
        let text = source(&module.path).unwrap_or_default();
        let position = |span: Span| line_column(&text, span.start);
        let _ = writeln!(out, "{}", relative(root, &module.path));
        let mut functions: Vec<&FunctionDecision> = module.log.functions.iter().collect();
        functions.sort_by_key(|f| f.span.start);
        for decision in functions {
            let (line, column) = position(decision.span);
            let _ = write!(
                out,
                "  {} ({line}:{column}): {}",
                decision.name.as_deref().unwrap_or("(anonymous)"),
                verdict(decision)
            );
            if let Some(island) = &decision.island {
                let _ = write!(out, " in island `{island}`");
            }
            out.push('\n');
            let _ = writeln!(out, "    why: {}", decision.reason);
            if !decision.failures.is_empty() {
                let _ = writeln!(out, "    failed rules:");
                for failure in &decision.failures {
                    let at = if let Some(path) = &failure.path {
                        let other = source(path).unwrap_or_default();
                        let (line, column) = line_column(&other, failure.span.start);
                        format!("{}:{line}:{column}", relative(root, path))
                    } else {
                        let (line, column) = position(failure.span);
                        format!("{line}:{column}")
                    };
                    let within = failure
                        .function
                        .as_ref()
                        .map(|f| format!("in `{f}`: "))
                        .unwrap_or_default();
                    let _ = writeln!(out, "      {at}  {within}{}", failure.message);
                }
            }
            if let Some(cost) = &decision.cost {
                cost_lines(&mut out, cost);
            }
            passes(&mut out, &module.log, decision.span, &position);
        }
        out.push('\n');
    }
    if out.is_empty() {
        out.push_str("no functions were considered\n");
    }
    out.trim_end().to_string() + "\n"
}

/// JS, WASM, forced or forbidden.
fn verdict(decision: &FunctionDecision) -> &'static str {
    match (decision.target, decision.directive) {
        (Target::Wasm, Some(Target::Wasm)) => "WASM (forced)",
        (Target::Js, Some(Target::Wasm)) => "JS (forced into WASM, but failed)",
        (Target::Js, Some(Target::Js)) => "JS (forbidden)",
        (Target::Wasm, _) => "WASM",
        (Target::Js, _) => "JS",
    }
}

fn cost_lines(out: &mut String, cost: &CostEstimate) {
    let comparison = if cost.profitable() { ">" } else { "≤" };
    let _ = writeln!(
        out,
        "    cost model: benefit {:.0} − cost {:.0} = {:.0} {comparison} threshold {:.0}",
        cost.benefit,
        cost.cost,
        cost.net(),
        cost.threshold
    );
    let _ = writeln!(
        out,
        "      benefit: {:.0} operations over {:.0} call(s)",
        cost.work, cost.calls
    );
    let _ = writeln!(
        out,
        "      cost: crossings {:.0} + marshalling {:.0} ({:.0} bytes) + instantiation {:.0}",
        cost.crossings, cost.marshalling, cost.bytes, cost.instantiation
    );
}

/// The pass events inside `span`, grouped by pass.
fn passes(
    out: &mut String,
    log: &ModuleLog,
    span: Span,
    position: &dyn Fn(Span) -> (usize, usize),
) {
    let mut applied: Vec<(&str, usize)> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for event in &log.passes {
        let Some(site) = event.span else {
            continue;
        };
        if site.start < span.start || site.end > span.end {
            continue;
        }
        match &event.outcome {
            PassOutcome::Applied => {
                match applied.iter_mut().find(|(pass, _)| *pass == event.pass) {
                    Some((_, count)) => *count += 1,
                    None => applied.push((&event.pass, 1)),
                }
            }
            PassOutcome::Skipped { reason } => {
                let (line, column) = position(site);
                let line = format!("{} skipped at {line}:{column}: {reason}", event.pass);
                if !skipped.contains(&line) {
                    skipped.push(line);
                }
            }
        }
    }
    if applied.is_empty() && skipped.is_empty() {
        return;
    }
    let _ = writeln!(out, "    passes:");
    if !applied.is_empty() {
        let list: Vec<String> = applied
            .iter()
            .map(|(pass, count)| format!("{pass} ×{count}"))
            .collect();
        let _ = writeln!(out, "      applied: {}", list.join(", "));
    }
    for line in skipped {
        let _ = writeln!(out, "      {line}");
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// 1-based line and column (in characters) of byte `offset`.
fn line_column(source: &str, offset: u32) -> (usize, usize) {
    let before = source
        .get(..(offset as usize).min(source.len()))
        .unwrap_or("");
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    (line, column)
}
