//! WASM islands through the facade: debug islands are built into `BuildOutput.islands`, and a
//! missing toolchain falls back to JS with one warning (or an error with `wasm.require`).

use std::path::{Path, PathBuf};

use trioxide::{
    IslandSource, Options, Project, Severity,
    wasm::{BuilderOptions, CargoIslandBuilder},
};

fn island_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/islands")
        .join(name)
        .canonicalize()
        .unwrap()
}

fn options() -> Options {
    Options {
        cache_dir: Some(Path::new(env!("CARGO_TARGET_TMPDIR")).join("island-cache")),
        ..Options::default()
    }
}

fn project(fixture: &Path, options: Options, islands: &[&str]) -> Project {
    let mut project = Project::new(fixture.to_path_buf(), options).unwrap();
    for name in islands {
        project.add_debug_island(IslandSource::load(&island_fixture(name).join("island")).unwrap());
    }
    project
}

/// A builder that finds no tools: an empty PATH and a cache that never gets filled.
fn no_toolchain(options: &Options) -> Box<CargoIslandBuilder> {
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let empty = tmp.join("empty-path");
    std::fs::create_dir_all(&empty).unwrap();
    let cache = tmp.join("unfilled-island-cache");
    Box::new(CargoIslandBuilder::new(BuilderOptions {
        path: Some(empty.into_os_string()),
        wasm_opt: options.wasm.wasm_opt,
        ..BuilderOptions::new(cache)
    }))
}

#[test]
fn builds_debug_islands_into_the_output() {
    let fixture = island_fixture("nbody");
    let mut project = project(&fixture, options(), &["nbody", "bitwise"]);
    let output = project.build(&[fixture.join("input.ts")]).unwrap();
    assert!(!output.has_errors());
    assert!(
        output.report.diagnostics.is_empty(),
        "{:?}",
        output.report.diagnostics
    );
    assert_eq!(output.modules.len(), 2);
    let ids: Vec<&str> = output
        .islands
        .iter()
        .map(|i| i.spec.id.0.as_str())
        .collect();
    assert_eq!(ids, ["nbody", "bitwise"]);
    for island in &output.islands {
        assert_eq!(&island.wasm[..4], b"\0asm");
        assert!(island.glue.contains(&format!(
            "new URL(\"{}\", import.meta.url)",
            trioxide::WASM_URL_PLACEHOLDER
        )));
    }
    assert!(
        output.islands[0]
            .glue
            .contains("export function advance(bodies, dt, steps)")
    );
}

#[test]
fn missing_toolchain_falls_back_with_one_warning() {
    let fixture = island_fixture("nbody");
    let options = options();
    let mut project = project(&fixture, options.clone(), &["nbody", "bitwise", "multi"]);
    project.set_island_builder(no_toolchain(&options));
    let output = project.build(&[fixture.join("input.ts")]).unwrap();
    assert!(!output.has_errors());
    assert_eq!(output.islands.len(), 0, "no island can be built");
    assert_eq!(output.modules.len(), 2, "the JS is still emitted");
    let [warning] = output.report.diagnostics.as_slice() else {
        panic!("expected one warning: {:?}", output.report.diagnostics);
    };
    assert_eq!(warning.severity, Severity::Warning);
    assert_eq!(warning.code.as_deref(), Some("trioxide(wasm-toolchain)"));
    assert!(
        warning
            .message
            .contains("3 WASM island(s) fall back to JS: nbody, bitwise, multi"),
        "{}",
        warning.message
    );
}

#[test]
fn require_wasm_turns_the_fallback_into_an_error() {
    let fixture = island_fixture("nbody");
    let mut options = options();
    options.wasm.require = true;
    let mut project = project(&fixture, options.clone(), &["nbody"]);
    project.set_island_builder(no_toolchain(&options));
    let output = project.build(&[fixture.join("input.ts")]).unwrap();
    assert!(output.has_errors());
    let [error] = output.report.diagnostics.as_slice() else {
        panic!("expected one error: {:?}", output.report.diagnostics);
    };
    assert_eq!(error.severity, Severity::Error);
    assert!(
        error.message.contains("--require-wasm"),
        "{}",
        error.message
    );
}

#[test]
fn no_wasm_skips_islands() {
    let fixture = island_fixture("nbody");
    let mut options = options();
    options.wasm.enabled = false;
    let mut project = project(&fixture, options.clone(), &["nbody"]);
    project.set_island_builder(no_toolchain(&options));
    let output = project.build(&[fixture.join("input.ts")]).unwrap();
    assert!(output.islands.is_empty() && output.report.diagnostics.is_empty());
}

#[test]
fn rust_islands_need_no_toolchain() {
    let fixture = island_fixture("particles");
    let options = options();
    let mut project = project(&fixture, options.clone(), &["particles"]);
    project.set_island_builder(no_toolchain(&options));
    let sources = project.rust_islands(&[fixture.join("input.ts")]).unwrap();
    assert_eq!(sources.len(), 1);
    assert!(sources[0].files["src/lib.rs"].contains("pub struct Body"));
    let runtime = project.island_runtime_dir().unwrap();
    assert!(runtime.join("src/lib.rs").is_file());
    assert!(
        sources[0]
            .manifest(&runtime)
            .contains("trioxide_rt = { path = ")
    );
}
