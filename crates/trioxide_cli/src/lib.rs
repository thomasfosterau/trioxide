//! The `trioxide` command-line tool (PLAN §7 "CLI").
//!
//! [`run`] is the whole tool: the `trioxide` binary calls it with the process arguments, and
//! the napi binding exposes it as `runCli(argv)` so the npm package's bin shares this one
//! implementation. [`discover_config`] and [`read_config`] are the config lookup it uses,
//! also shared with the Vite plugin through the binding.

#![expect(clippy::print_stderr, reason = "a CLI reports on stderr")]

mod config;
mod render;
mod watch;

use std::{
    ffi::OsString,
    io::Write as _,
    path::{Path, PathBuf},
};

use clap::{Args, Parser, Subcommand, ValueEnum};
use trioxide::{ExplainQuery, ModuleOutput, Options, Project};
use trioxide_wasm::toolchain::Toolchain;

pub use config::{discover_config, read_config};
use render::Renderer;

#[derive(Debug, Parser)]
#[command(
    name = "trioxide",
    version,
    about = "A TypeScript-aware optimising compiler: readable JS or Rust/WASM, per function."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    #[command(flatten)]
    global: GlobalArgs,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Compile one file and print the JavaScript to stdout.
    Transform {
        /// The TypeScript file.
        file: PathBuf,
    },
    /// Compile the program reachable from the entries into a directory.
    Build {
        /// Entry modules.
        #[arg(required = true)]
        entries: Vec<PathBuf>,
        /// Where to write the output, mirroring the layout under the project root.
        #[arg(long, default_value = "dist")]
        out_dir: PathBuf,
        /// Rebuild whenever a module in the graph or the config changes.
        #[arg(long, short)]
        watch: bool,
    },
    /// Explain partitioning and optimisation decisions for a file or function (`file[:fn]`).
    Explain {
        /// `file` or `file:function`; everything if omitted.
        target: Option<String>,
    },
    /// Report whether the Rust toolchain needed for WASM islands is installed.
    CheckToolchain {
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Strip types only, with no analysis or optimisation (the differential suite's baseline).
    #[command(hide = true)]
    Strip {
        /// The TypeScript file.
        file: PathBuf,
    },
}

#[derive(Debug, Args)]
struct GlobalArgs {
    /// A `trioxide.json` / `trioxide.jsonc` file. Default: the nearest one at or above the
    /// project root.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// The project root. Default: the config file's directory, else the nearest directory
    /// above the input containing a tsconfig or trioxide config, else the working directory.
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    /// How aggressively functions move to WebAssembly.
    #[arg(long, global = true, value_enum)]
    mode: Option<ModeArg>,
    /// Closed-world (`app`) or open-world (`library`) assumption.
    #[arg(long, global = true, value_enum)]
    world: Option<WorldArg>,
    /// Treat declared TypeScript types as proof.
    #[arg(long, global = true)]
    trust_types: bool,
    /// Never produce WASM islands.
    #[arg(long, global = true)]
    no_wasm: bool,
    /// What to emit.
    #[arg(long, global = true, value_enum, default_value = "js")]
    emit: Emit,
    /// When to colour diagnostics. `auto` colours a terminal unless `NO_COLOR` is set.
    #[arg(long, global = true, value_enum, default_value = "auto")]
    color: ColorArg,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ModeArg {
    JsOnly,
    Auto,
    WasmPreferred,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum WorldArg {
    App,
    Library,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Emit {
    Js,
    Rust,
    Wasm,
    Report,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ColorArg {
    Auto,
    Always,
    Never,
}

/// A failure that ends the command.
enum Failure {
    /// Bad usage or an unimplemented feature (exit code 2).
    Usage(String),
    /// trioxide failed or the input has errors (exit code 1).
    Error(String),
}

impl From<trioxide::Error> for Failure {
    fn from(error: trioxide::Error) -> Self {
        Self::Error(error.to_string())
    }
}

/// Runs the CLI with `args` (the first one is the program name) and returns the exit code:
/// 0 on success, 1 when trioxide failed or the input has errors, 2 for bad usage.
///
/// Output goes to the process's stdout and stderr.
pub fn run<I, T>(args: I) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            return if error.use_stderr() { 2 } else { 0 };
        }
    };
    let renderer = Renderer::new(match cli.global.color {
        ColorArg::Auto => None,
        ColorArg::Always => Some(true),
        ColorArg::Never => Some(false),
    });
    match execute(&cli, &renderer) {
        Ok(()) => 0,
        Err(Failure::Usage(message)) => {
            renderer.error(&message);
            2
        }
        Err(Failure::Error(message)) => {
            if !message.is_empty() {
                renderer.error(&message);
            }
            1
        }
    }
}

fn execute(cli: &Cli, renderer: &Renderer) -> Result<(), Failure> {
    let global = &cli.global;
    match global.emit {
        Emit::Js => {}
        Emit::Rust | Emit::Wasm | Emit::Report => {
            return Err(not_implemented(&format!(
                "--emit={}",
                emit_name(global.emit)
            )));
        }
    }

    match &cli.command {
        Command::Transform { file } => {
            let mut project = open_project(global, Some(file), renderer)?;
            let out = project.transform(file)?;
            renderer.module_diagnostics(&out);
            if out.has_errors() {
                return Err(Failure::Error(String::new()));
            }
            print_stdout(&out.code);
            Ok(())
        }
        Command::Strip { file } => {
            let (root, options, _) = resolve_config(global, Some(file), renderer)?;
            let source = std::fs::read_to_string(file)
                .map_err(|error| Failure::Error(format!("{}: {error}", file.display())))?;
            let out = trioxide::strip_types(&root, file, &source, &options)?;
            renderer.module_diagnostics(&out);
            if out.has_errors() {
                return Err(Failure::Error(String::new()));
            }
            print_stdout(&out.code);
            Ok(())
        }
        Command::Build {
            entries,
            out_dir,
            watch,
        } => {
            if *watch {
                return watch::watch(global, entries, out_dir, renderer);
            }
            let mut project = open_project(global, entries.first(), renderer)?;
            build_once(&mut project, entries, out_dir, renderer)
        }
        Command::Explain { target } => {
            let _query: ExplainQuery = target
                .as_deref()
                .unwrap_or_default()
                .parse()
                .unwrap_or_default();
            Err(not_implemented("trioxide explain"))
        }
        Command::CheckToolchain { json } => check_toolchain(*json, renderer),
    }
}

/// Builds `entries` and writes every module under `out_dir`.
fn build_once(
    project: &mut Project,
    entries: &[PathBuf],
    out_dir: &Path,
    renderer: &Renderer,
) -> Result<(), Failure> {
    let output = project.build(entries)?;
    for diagnostic in &output.report.diagnostics {
        renderer.diagnostic(diagnostic, None);
    }
    for module in &output.modules {
        renderer.module_diagnostics(module);
    }
    if output.has_errors() {
        return Err(Failure::Error("build failed".into()));
    }
    for module in &output.modules {
        write_module(project.root(), out_dir, module)?;
    }
    Ok(())
}

fn check_toolchain(json: bool, renderer: &Renderer) -> Result<(), Failure> {
    let toolchain = Toolchain::detect();
    if json {
        let text = serde_json::to_string_pretty(&serde_json::json!({
            "toolchain": toolchain,
            "missing": toolchain.missing(),
            "canBuildIslands": toolchain.can_build_islands(),
        }))
        .expect("toolchain serialises");
        print_stdout(&format!("{text}\n"));
    } else {
        renderer.toolchain(&toolchain);
    }
    if toolchain.can_build_islands() {
        Ok(())
    } else {
        Err(Failure::Error(String::new()))
    }
}

fn not_implemented(what: &str) -> Failure {
    Failure::Usage(format!("{what} is not yet implemented"))
}

fn emit_name(emit: Emit) -> &'static str {
    match emit {
        Emit::Js => "js",
        Emit::Rust => "rust",
        Emit::Wasm => "wasm",
        Emit::Report => "report",
    }
}

/// Finds the project root, options and config file from the flags, the config files above
/// the root and `input`'s location.
fn resolve_config(
    global: &GlobalArgs,
    input: Option<&PathBuf>,
    renderer: &Renderer,
) -> Result<(PathBuf, Options, Option<PathBuf>), Failure> {
    let cwd = std::env::current_dir().map_err(|error| Failure::Error(error.to_string()))?;
    let root = match (&global.root, &global.config) {
        (Some(root), _) => cwd.join(root),
        (None, Some(config)) => config
            .parent()
            .map_or_else(|| cwd.clone(), |dir| cwd.join(dir)),
        (None, None) => input
            .and_then(|input| discover_root(&cwd.join(input)))
            .unwrap_or_else(|| cwd.clone()),
    };
    let config = global
        .config
        .as_ref()
        .map(|config| cwd.join(config))
        .or_else(|| discover_config(&root));
    let mut options = match &config {
        Some(path) => Options::from_file(path)?,
        None => Options::default(),
    };

    let mut ignored = Vec::new();
    if let Some(mode) = global.mode {
        options.mode = match mode {
            ModeArg::JsOnly => trioxide::Mode::JsOnly,
            ModeArg::Auto => trioxide::Mode::Auto,
            ModeArg::WasmPreferred => trioxide::Mode::WasmPreferred,
        };
        ignored.push("--mode");
    }
    if let Some(world) = global.world {
        options.world = Some(match world {
            WorldArg::App => trioxide::World::App,
            WorldArg::Library => trioxide::World::Library,
        });
        ignored.push("--world");
    }
    if global.trust_types {
        options.trust_types = true;
        ignored.push("--trust-types");
    }
    if global.no_wasm {
        options.wasm.enabled = false;
        ignored.push("--no-wasm");
    }
    for flag in ignored {
        renderer.warning(&format!("{flag} is not yet implemented and has no effect"));
    }
    Ok((root, options, config))
}

/// The nearest directory at or above `file` with a tsconfig or trioxide config.
fn discover_root(file: &Path) -> Option<PathBuf> {
    file.parent()?
        .ancestors()
        .find(|dir| dir.join("tsconfig.json").is_file() || Options::find_config(dir).is_some())
        .map(Path::to_path_buf)
}

fn open_project(
    global: &GlobalArgs,
    input: Option<&PathBuf>,
    renderer: &Renderer,
) -> Result<Project, Failure> {
    let (root, options, _) = resolve_config(global, input, renderer)?;
    Ok(Project::new(root, options)?)
}

fn print_stdout(text: &str) {
    let mut stdout = std::io::stdout().lock();
    // A closed pipe (`trioxide transform … | head`) is not an error worth reporting.
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

/// Writes a module (and its sourcemap) under `out_dir`, mirroring its path under `root`.
fn write_module(root: &Path, out_dir: &Path, module: &ModuleOutput) -> Result<(), Failure> {
    let relative = module.path.strip_prefix(root).map_or_else(
        |_| PathBuf::from("_external").join(module.path.file_name().unwrap_or_default()),
        Path::to_path_buf,
    );
    let destination = out_dir.join(output_name(&relative));
    let io_error =
        |error: std::io::Error| Failure::Error(format!("{}: {error}", destination.display()));
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(io_error)?;
    }
    let mut code = module.code.clone();
    if let Some(map) = &module.map {
        let map_path = destination.with_extension(format!(
            "{}.map",
            destination
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("js")
        ));
        std::fs::write(&map_path, map.as_json()).map_err(io_error)?;
        let name = map_path.file_name().unwrap_or_default().to_string_lossy();
        code.push_str("//# sourceMappingURL=");
        code.push_str(&name);
        code.push('\n');
    }
    std::fs::write(&destination, code).map_err(io_error)
}

/// `a.ts` → `a.js`, `a.mts` → `a.mjs`, `a.cts` → `a.cjs`, `a.tsx` → `a.js`.
fn output_name(path: &Path) -> PathBuf {
    let extension = match path.extension().and_then(|ext| ext.to_str()) {
        Some("mts" | "mjs") => "mjs",
        Some("cts" | "cjs") => "cjs",
        _ => "js",
    };
    path.with_extension(extension)
}
