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
    fmt::Write as _,
    io::Write as _,
    path::{Path, PathBuf},
};

use clap::{Args, Parser, Subcommand, ValueEnum};
use trioxide::{
    ExplainQuery, IslandArtifact, IslandSource, ModuleOutput, Options, Project,
    WASM_URL_PLACEHOLDER,
};
use trioxide_wasm::toolchain::Toolchain;

/// Where `trioxide build` writes island glue, `.wasm` files and (`--emit=rust`) crates,
/// under the output directory.
const ISLANDS_DIR: &str = "_islands";

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
    /// Fail instead of falling back to JS when an island can't be built (e.g. no Rust
    /// toolchain).
    #[arg(long, global = true)]
    require_wasm: bool,
    /// Build the island in this directory (`island.json` plus `src/`) as if the partitioner
    /// had chosen it. A debug and test hook until islands are selected automatically.
    #[arg(long, global = true, hide = true, value_name = "DIR")]
    debug_island: Vec<PathBuf>,
    /// What to emit. `facts` prints the analysis facts of a `transform`ed file.
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
    Facts,
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
    let is_build = matches!(cli.command, Command::Build { .. });
    match global.emit {
        Emit::Js => {}
        Emit::Facts => {
            let Command::Transform { file } = &cli.command else {
                return Err(Failure::Usage(
                    "--emit=facts only applies to `trioxide transform`".into(),
                ));
            };
            let file = std::path::absolute(file)
                .map_err(|error| Failure::Error(format!("{}: {error}", file.display())))?;
            let mut project = open_project(global, Some(&file), renderer)?;
            print_stdout(&project.facts(&file)?);
            return Ok(());
        }
        Emit::Rust | Emit::Wasm if is_build => {}
        Emit::Rust if matches!(cli.command, Command::Transform { .. }) => {
            let Command::Transform { file } = &cli.command else {
                unreachable!("matched above")
            };
            let file = std::path::absolute(file)
                .map_err(|error| Failure::Error(format!("{}: {error}", file.display())))?;
            let mut project = open_project(global, Some(&file), renderer)?;
            let islands = project.rust_islands(std::slice::from_ref(&file))?;
            let report = project.explain(&ExplainQuery::default());
            for diagnostic in &report.diagnostics {
                renderer.project_diagnostic(diagnostic);
            }
            if report
                .diagnostics
                .iter()
                .any(trioxide::Diagnostic::is_error)
            {
                return Err(Failure::Error(String::new()));
            }
            let mut text = String::new();
            for island in &islands {
                let _ = writeln!(
                    text,
                    "// {}/island.json\n{}",
                    island.spec.id,
                    island.spec.to_json()
                );
                for (path, source) in &island.files {
                    let _ = write!(text, "\n// {}/{path}\n{source}", island.spec.id);
                }
            }
            print_stdout(&text);
            return Ok(());
        }
        Emit::Rust | Emit::Wasm | Emit::Report => {
            return Err(not_implemented(&format!(
                "--emit={}{}",
                emit_name(global.emit),
                if is_build {
                    ""
                } else {
                    " outside `trioxide build`"
                }
            )));
        }
    }

    match &cli.command {
        Command::Transform { file } => {
            // Resolved once, against the working directory (not again against the root).
            let file = std::path::absolute(file)
                .map_err(|error| Failure::Error(format!("{}: {error}", file.display())))?;
            let mut project = open_project(global, Some(&file), renderer)?;
            let out = project.transform(&file)?;
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
            if global.emit == Emit::Rust {
                let runtime = project.island_runtime_dir()?;
                for island in project.rust_islands(entries)? {
                    write_island_crate(out_dir, &island, &runtime)?;
                }
                return Ok(());
            }
            build_once(&mut project, entries, out_dir, global.emit, renderer)
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

/// Builds `entries` and writes every module (unless `emit` is `wasm`) and island under
/// `out_dir`.
fn build_once(
    project: &mut Project,
    entries: &[PathBuf],
    out_dir: &Path,
    emit: Emit,
    renderer: &Renderer,
) -> Result<(), Failure> {
    let output = project.build(entries)?;
    for diagnostic in &output.report.diagnostics {
        renderer.project_diagnostic(diagnostic);
    }
    for module in &output.modules {
        renderer.module_diagnostics(module);
    }
    if output.has_errors() {
        return Err(Failure::Error("build failed".into()));
    }
    if emit == Emit::Js {
        for module in &output.modules {
            write_module(project.root(), out_dir, module)?;
        }
    }
    for island in &output.islands {
        write_island(project.root(), out_dir, island)?;
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
        Emit::Facts => "facts",
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
    }
    if global.trust_types {
        options.trust_types = true;
    }
    if global.no_wasm {
        options.wasm.enabled = false;
    }
    if global.require_wasm {
        options.wasm.require = true;
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
    let mut project = Project::new(root, options)?;
    add_debug_islands(global, &mut project)?;
    Ok(project)
}

/// Adds the `--debug-island` directories to `project`.
fn add_debug_islands(global: &GlobalArgs, project: &mut Project) -> Result<(), Failure> {
    for dir in &global.debug_island {
        let island = IslandSource::load(dir).map_err(|error| {
            Failure::Error(format!("--debug-island {}: {error}", dir.display()))
        })?;
        project.add_debug_island(island);
    }
    Ok(())
}

fn print_stdout(text: &str) {
    let mut stdout = std::io::stdout().lock();
    // A closed pipe (`trioxide transform … | head`) is not an error worth reporting.
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

/// Writes a module (and its sourcemap) under `out_dir`, mirroring its path under `root`.
/// Where `trioxide build` writes the module at `path`, relative to the output directory.
fn output_path(root: &Path, path: &Path) -> PathBuf {
    let relative = path.strip_prefix(root).map_or_else(
        |_| PathBuf::from("_external").join(path.file_name().unwrap_or_default()),
        Path::to_path_buf,
    );
    output_name(&relative)
}

fn write_module(root: &Path, out_dir: &Path, module: &ModuleOutput) -> Result<(), Failure> {
    let relative = output_path(root, &module.path);
    let destination = out_dir.join(&relative);
    let io_error =
        |error: std::io::Error| Failure::Error(format!("{}: {error}", destination.display()));
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(io_error)?;
    }
    let depth = relative.components().count().saturating_sub(1);
    let mut code = link_islands(&module.code, &module.islands, depth);
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

/// Points a module's glue imports at `<out_dir>/_islands/<id>.js`, from a module `depth`
/// directories below the output directory.
fn link_islands(code: &str, islands: &[trioxide::IslandRef], depth: usize) -> String {
    let mut code = code.to_string();
    let prefix = if depth == 0 {
        "./".to_string()
    } else {
        "../".repeat(depth)
    };
    for island in islands {
        let target = format!("{prefix}{ISLANDS_DIR}/{}.js", island.id);
        // Whole string literals only: one id can be a prefix of another (`k1`, `k10`).
        let escaped = island.specifier.replace('\0', "\\0");
        for specifier in [&escaped, &island.specifier] {
            for quote in ['"', '\'', '`'] {
                code = code.replace(
                    &format!("{quote}{specifier}{quote}"),
                    &format!("{quote}{target}{quote}"),
                );
            }
        }
    }
    code
}

/// Writes an island's glue and `.wasm` into `<out_dir>/_islands/`, pointing the glue at the
/// `.wasm` next to it.
fn write_island(root: &Path, out_dir: &Path, island: &IslandArtifact) -> Result<(), Failure> {
    let dir = out_dir.join(ISLANDS_DIR);
    let mut glue = island.glue.replace(
        WASM_URL_PLACEHOLDER,
        &format!("./{}", island.wasm_file_name()),
    );
    // The glue names its fallback by source path; point it at the compiled module.
    if let Some(fallback) = &island.spec.fallback
        && Path::new(fallback).is_absolute()
    {
        let relative = output_path(root, Path::new(fallback));
        let parts: Vec<String> = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let quote = |text: &str| serde_json::to_string(text).expect("strings serialise");
        glue = glue.replace(&quote(fallback), &quote(&format!("../{}", parts.join("/"))));
    }
    write_file(&dir.join(island.glue_file_name()), glue.as_bytes())?;
    write_file(&dir.join(island.wasm_file_name()), &island.wasm)
}

/// Writes an island's crate (`island.json`, a standalone `Cargo.toml`, `src/`) into
/// `<out_dir>/_islands/<id>/`.
fn write_island_crate(
    out_dir: &Path,
    island: &IslandSource,
    runtime: &Path,
) -> Result<(), Failure> {
    let dir = out_dir.join(ISLANDS_DIR).join(&island.spec.id.0);
    write_file(&dir.join("island.json"), island.spec.to_json().as_bytes())?;
    write_file(&dir.join("Cargo.toml"), island.manifest(runtime).as_bytes())?;
    for (path, source) in &island.files {
        write_file(&dir.join(path), source.as_bytes())?;
    }
    Ok(())
}

fn write_file(path: &Path, contents: &[u8]) -> Result<(), Failure> {
    let io_error = |error: std::io::Error| Failure::Error(format!("{}: {error}", path.display()));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_error)?;
    }
    std::fs::write(path, contents).map_err(io_error)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn island_links_match_whole_specifiers() {
        let islands: Vec<trioxide::IslandRef> = ["k1", "k10"]
            .into_iter()
            .map(|id| {
                let id = trioxide::IslandId(id.to_string());
                trioxide::IslandRef {
                    specifier: id.glue_specifier(),
                    id,
                }
            })
            .collect();
        let code = "import { a } from \"\\0trioxide:island/k10\";\nimport { b } from '\\0trioxide:island/k1';\n";
        assert_eq!(
            link_islands(code, &islands, 1),
            "import { a } from \"../_islands/k10.js\";\nimport { b } from '../_islands/k1.js';\n"
        );
    }
}
