//! Finding the Rust toolchain that island builds need (PLAN D6).
//!
//! Islands need `cargo`, `rustc` and the `wasm32-unknown-unknown` standard library;
//! `wasm-opt` is optional. [`detect`] reports on all of them, for `trioxide check-toolchain`.
//! The island builder uses the same functions lazily, so a fully cached build runs nothing
//! but `rustc -vV`.
//!
//! Tools are looked up on `PATH` only (never `$CARGO` or `$RUSTC`), and the lookup takes the
//! `PATH` value explicitly so callers and tests can substitute it. Commands run in the
//! directory the build will run in, because rustup picks the toolchain from the nearest
//! `rust-toolchain.toml`.

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Command,
};

/// The target islands compile to.
pub const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// Where and how to look for tools.
#[derive(Debug, Clone, Default)]
pub struct Probe {
    /// The `PATH` to search; `None` uses the process's `PATH`.
    pub path: Option<OsString>,
    /// The directory commands run in; `None` uses the working directory.
    pub cwd: Option<PathBuf>,
}

impl Probe {
    /// The `PATH` value in effect.
    pub fn path_var(&self) -> OsString {
        self.path
            .clone()
            .or_else(|| std::env::var_os("PATH"))
            .unwrap_or_default()
    }

    /// Finds an executable named `name` on the `PATH`.
    pub fn find(&self, name: &str) -> Option<PathBuf> {
        let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
        std::env::split_paths(&self.path_var())
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(|dir| dir.join(&file))
            .find(|path| is_executable(path))
    }

    /// A command for `program` with this probe's `PATH` and working directory.
    pub fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command.env("PATH", self.path_var());
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        command
    }

    /// Runs `program args…` and returns its trimmed stdout, or why it failed.
    pub fn output(&self, program: &Path, args: &[&str]) -> Result<String, String> {
        let output = self
            .command(program)
            .args(args)
            .output()
            .map_err(|error| format!("could not run {}: {error}", program.display()))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(format!(
                "`{} {}` failed: {}",
                program.display(),
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// A tool that was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    /// Its path.
    pub path: PathBuf,
    /// Its version output (`rustc -vV` gives several lines).
    pub version: String,
}

/// `rustc` plus what it says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rustc {
    /// The tool; `version` is the full `rustc -vV` output, used in cache keys.
    pub tool: Tool,
    /// The `release:` line, e.g. `1.99.1`.
    pub release: String,
    /// The sysroot (`rustc --print sysroot`).
    pub sysroot: PathBuf,
}

impl Rustc {
    /// Whether the sysroot has the wasm32 standard library.
    pub fn has_wasm_target(&self) -> bool {
        self.sysroot
            .join("lib/rustlib")
            .join(WASM_TARGET)
            .join("lib")
            .is_dir()
    }
}

/// Finds `rustc` and asks for its version and sysroot.
pub fn rustc(probe: &Probe) -> Result<Rustc, String> {
    let path = probe
        .find("rustc")
        .ok_or_else(|| "`rustc` was not found on PATH".to_string())?;
    let version = probe.output(&path, &["-vV"])?;
    let release = version
        .lines()
        .find_map(|line| line.strip_prefix("release: "))
        .unwrap_or("unknown")
        .to_string();
    let sysroot = PathBuf::from(probe.output(&path, &["--print", "sysroot"])?);
    Ok(Rustc {
        tool: Tool { path, version },
        release,
        sysroot,
    })
}

/// Finds `cargo` and asks for its version.
pub fn cargo(probe: &Probe) -> Result<Tool, String> {
    let path = probe
        .find("cargo")
        .ok_or_else(|| "`cargo` was not found on PATH".to_string())?;
    let version = probe.output(&path, &["--version"])?;
    Ok(Tool { path, version })
}

/// Finds `wasm-opt` (Binaryen) and asks for its version.
pub fn wasm_opt(probe: &Probe) -> Option<Tool> {
    let path = probe.find("wasm-opt")?;
    let version = probe.output(&path, &["--version"]).ok()?;
    Some(Tool { path, version })
}

/// The state of every tool island builds use.
#[derive(Debug, Clone)]
pub struct ToolchainReport {
    /// `cargo`, or why it is unusable.
    pub cargo: Result<Tool, String>,
    /// `rustc`, or why it is unusable.
    pub rustc: Result<Rustc, String>,
    /// Whether the `wasm32-unknown-unknown` standard library is installed for that `rustc`.
    pub wasm_target: bool,
    /// `wasm-opt`, if it is on `PATH` (optional).
    pub wasm_opt: Option<Tool>,
}

impl ToolchainReport {
    /// Whether islands can be built.
    pub fn is_ready(&self) -> bool {
        self.problems().is_empty()
    }

    /// What is missing, with how to fix it. Empty when [`is_ready`](Self::is_ready).
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if let Err(error) = &self.cargo {
            problems.push(format!("{error}; install Rust from https://rustup.rs"));
        }
        match &self.rustc {
            Err(error) => problems.push(format!("{error}; install Rust from https://rustup.rs")),
            Ok(rustc) if !self.wasm_target => problems.push(format!(
                "the {WASM_TARGET} target is not installed for rustc {}; run `rustup target add {WASM_TARGET}`",
                rustc.release
            )),
            Ok(_) => {}
        }
        problems
    }
}

/// Reports on every tool.
pub fn detect(probe: &Probe) -> ToolchainReport {
    let rustc = rustc(probe);
    ToolchainReport {
        cargo: cargo(probe),
        wasm_target: rustc.as_ref().is_ok_and(Rustc::has_wasm_target),
        rustc,
        wasm_opt: wasm_opt(probe),
    }
}

/// Environment variables that would change how cargo builds an island; the builder removes
/// them so builds are reproducible and match their cache key.
pub const SCRUBBED_ENV: &[&str] = &[
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_BUILD_RUSTFLAGS",
    "RUSTDOCFLAGS",
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET",
    "CARGO_BUILD_TARGET_DIR",
    "CARGO_BUILD_JOBS",
    "CARGO_INCREMENTAL",
    "CARGO_PROFILE_RELEASE_OPT_LEVEL",
    "CARGO_PROFILE_RELEASE_LTO",
    "CARGO_PROFILE_RELEASE_CODEGEN_UNITS",
    "CARGO_PROFILE_RELEASE_PANIC",
    "CARGO_PROFILE_RELEASE_DEBUG",
    "CARGO_PROFILE_RELEASE_STRIP",
    "CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS",
    "CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS",
];

/// Whether `name` is an environment variable a nested cargo would misread, such as
/// `CARGO_MANIFEST_DIR` set by an enclosing `cargo test`.
pub(crate) fn is_scrubbed(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        SCRUBBED_ENV.contains(&name)
            || name.starts_with("CARGO_TARGET_")
            || name.starts_with("CARGO_PKG_")
            || name == "CARGO_MANIFEST_DIR"
            || name == "CARGO_PRIMARY_PACKAGE"
            || name == "CARGO_CRATE_NAME"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tools_are_reported() {
        let empty = tempfile::tempdir().unwrap();
        let probe = Probe {
            path: Some(empty.path().as_os_str().to_owned()),
            cwd: None,
        };
        let report = detect(&probe);
        assert!(!report.is_ready());
        let problems = report.problems().join("\n");
        assert!(problems.contains("`cargo` was not found"), "{problems}");
        assert!(problems.contains("`rustc` was not found"), "{problems}");
        assert!(report.wasm_opt.is_none());
    }

    #[test]
    fn finds_the_real_toolchain() {
        let report = detect(&Probe::default());
        assert!(report.is_ready(), "{:?}", report.problems());
        let rustc = report.rustc.unwrap();
        assert!(
            rustc.tool.version.contains("release: "),
            "{}",
            rustc.tool.version
        );
    }
}
