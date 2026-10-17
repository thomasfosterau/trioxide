use std::{io::IsTerminal as _, path::Path};

use oxc_diagnostics::{GraphicalReportHandler, GraphicalTheme, NamedSource};
use trioxide::{Diagnostic, ModuleOutput, Severity};
use trioxide_wasm::toolchain::{Tool, Toolchain};

const RED: &str = "\x1b[1;31m";
const YELLOW: &str = "\x1b[1;33m";
const GREEN: &str = "\x1b[1;32m";
const CYAN: &str = "\x1b[1;36m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

/// Prints messages and diagnostics to stderr, coloured when wanted.
pub struct Renderer {
    color: bool,
}

impl Renderer {
    /// `color`: forced on or off, or `None` to colour a terminal unless `NO_COLOR` is set
    /// (`FORCE_COLOR` forces it on).
    pub fn new(color: Option<bool>) -> Self {
        let env_set = |name| std::env::var_os(name).is_some_and(|value| !value.is_empty());
        let color = color.unwrap_or_else(|| {
            if env_set("FORCE_COLOR") {
                std::env::var_os("FORCE_COLOR").is_some_and(|value| value != "0")
            } else {
                !env_set("NO_COLOR") && std::io::stderr().is_terminal()
            }
        });
        Self { color }
    }

    fn paint(&self, style: &str, text: &str) -> String {
        if self.color {
            format!("{style}{text}{RESET}")
        } else {
            text.to_string()
        }
    }

    pub fn error(&self, message: &str) {
        eprintln!("{}: {message}", self.paint(RED, "error"));
    }

    pub fn warning(&self, message: &str) {
        eprintln!("{}: {message}", self.paint(YELLOW, "warning"));
    }

    pub fn info(&self, message: &str) {
        eprintln!("{}", self.paint(CYAN, message));
    }

    pub fn module_diagnostics(&self, out: &ModuleOutput) {
        if out.diagnostics.is_empty() {
            return;
        }
        let source = std::fs::read_to_string(&out.path).ok();
        for diagnostic in &out.diagnostics {
            self.diagnostic(
                diagnostic,
                source.as_deref().map(|source| (&*out.path, source)),
            );
        }
    }

    /// Prints `diagnostic`, with a code frame when its labels point into `source`.
    pub fn diagnostic(&self, diagnostic: &Diagnostic, source: Option<(&Path, &str)>) {
        let source = source.filter(|(_, source)| !source.is_empty());
        let (Some((path, source)), false) = (source, diagnostic.labels.is_empty()) else {
            let line = diagnostic.to_string();
            let severity = match diagnostic.severity {
                Severity::Error => ("error", RED),
                Severity::Warning => ("warning", YELLOW),
                Severity::Advice => ("advice", CYAN),
            };
            // Colour the severity word in `path: severity[code]: message`.
            let painted = line.replacen(severity.0, &self.paint(severity.1, severity.0), 1);
            eprintln!("{painted}");
            return;
        };
        let theme = if self.color {
            GraphicalTheme::unicode()
        } else {
            GraphicalTheme::none()
        };
        let report = diagnostic.to_oxc().with_source_code(NamedSource::new(
            path.display().to_string(),
            source.to_string(),
        ));
        let mut rendered = String::new();
        let _ = GraphicalReportHandler::new_themed(theme)
            .with_width(80)
            .with_links(false)
            .render_report(&mut rendered, report.as_ref());
        eprint!("{rendered}");
    }

    pub fn toolchain(&self, toolchain: &Toolchain) {
        let mark = |ok: bool| {
            if ok {
                self.paint(GREEN, "ok      ")
            } else {
                self.paint(RED, "missing ")
            }
        };
        let tool = |tool: &Tool, label: &str| {
            let detail = match (&tool.version, &tool.path) {
                (Some(version), Some(path)) => {
                    format!("{version} {}", self.paint(DIM, &path.display().to_string()))
                }
                (Some(version), None) => version.clone(),
                (None, Some(path)) => format!("found at {} but did not run", path.display()),
                (None, None) => "not found on PATH".to_string(),
            };
            eprintln!("{label:<10} {} {detail}", mark(tool.is_available()));
        };
        tool(&toolchain.rustc, "rustc");
        tool(&toolchain.cargo, "cargo");
        eprintln!(
            "{:<10} {} {}",
            "wasm32",
            mark(toolchain.wasm32_target),
            if toolchain.wasm32_target {
                "wasm32-unknown-unknown installed"
            } else {
                "wasm32-unknown-unknown not installed"
            }
        );
        let wasm_opt = &toolchain.wasm_opt;
        if wasm_opt.is_available() {
            tool(wasm_opt, "wasm-opt");
        } else {
            eprintln!(
                "{:<10} {} not found (optional: install binaryen for smaller, faster islands)",
                "wasm-opt",
                self.paint(YELLOW, "optional")
            );
        }
        let missing = toolchain.missing();
        if missing.is_empty() {
            eprintln!("\nWASM islands can be built.");
        } else {
            eprintln!("\nWASM islands fall back to JS until these are installed:");
            for item in missing {
                eprintln!("  - {}: {}", item.what, item.fix);
            }
        }
    }
}
