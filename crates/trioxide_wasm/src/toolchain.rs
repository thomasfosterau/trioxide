//! Detecting the Rust toolchain that island builds need (PLAN D6).
//!
//! [`Toolchain::detect`] probes `rustc`, `cargo`, the `wasm32-unknown-unknown` target and
//! `wasm-opt`. `trioxide check-toolchain` prints the result. The island builder uses the same
//! [`Probe`] lazily, through [`rustc`], [`cargo`] and [`wasm_opt`], so a fully cached build
//! runs nothing but `rustc -vV`.
//!
//! Tools are looked up on `PATH` only (never `$CARGO` or `$RUSTC`), and the probe can replace
//! `PATH` so callers and tests can substitute it. Commands run in the probe's directory,
//! because rustup picks the toolchain from the nearest `rust-toolchain.toml`.

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Command,
};

use serde::Serialize;

/// The target islands compile to.
pub const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// Where to look for tools. The default probes the current process's `PATH` and working
/// directory, which also decides which `rust-toolchain.toml` rustup honours.
#[derive(Debug, Clone, Default)]
pub struct Probe {
    /// Replaces `PATH` for the lookup and for the probed commands.
    pub path: Option<OsString>,
    /// The directory the probed commands run in.
    pub cwd: Option<PathBuf>,
}

/// One external tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    /// The command name, e.g. `rustc`.
    pub name: String,
    /// Where it was found on `PATH`, if it was.
    pub path: Option<PathBuf>,
    /// The first line of `<name> --version`, if it ran.
    pub version: Option<String>,
}

impl Tool {
    /// Whether the tool ran.
    pub fn is_available(&self) -> bool {
        self.version.is_some()
    }
}

/// What island builds can use on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Toolchain {
    /// The Rust compiler (required).
    pub rustc: Tool,
    /// Cargo (required).
    pub cargo: Tool,
    /// Whether the standard library for [`WASM_TARGET`] is installed (required).
    pub wasm32_target: bool,
    /// The rustc sysroot the target was looked up in.
    pub sysroot: Option<PathBuf>,
    /// Binaryen's optimiser (optional; islands are smaller and faster with it).
    pub wasm_opt: Tool,
}

impl Toolchain {
    /// Probes the current environment.
    pub fn detect() -> Self {
        Self::detect_with(&Probe::default())
    }

    /// Probes with an explicit `PATH` and working directory.
    pub fn detect_with(probe: &Probe) -> Self {
        let rustc = probe.tool("rustc");
        let sysroot = rustc
            .is_available()
            .then(|| probe.run("rustc", &["--print", "sysroot"]))
            .flatten()
            .map(PathBuf::from);
        let wasm32_target = sysroot.as_deref().is_some_and(has_wasm_target);
        Self {
            rustc,
            cargo: probe.tool("cargo"),
            wasm32_target,
            sysroot,
            wasm_opt: probe.tool("wasm-opt"),
        }
    }

    /// Whether everything required to build islands is present.
    pub fn can_build_islands(&self) -> bool {
        self.missing().is_empty()
    }

    /// What is missing for island builds, each with how to install it. `wasm-opt` is
    /// optional and not listed.
    pub fn missing(&self) -> Vec<Missing> {
        let mut missing = Vec::new();
        if !self.rustc.is_available() || !self.cargo.is_available() {
            missing.push(Missing {
                what: if self.rustc.is_available() {
                    "cargo"
                } else if self.cargo.is_available() {
                    "rustc"
                } else {
                    "rustc and cargo"
                },
                fix: "install Rust with rustup: https://rustup.rs",
            });
        }
        if self.rustc.is_available() && !self.wasm32_target {
            missing.push(Missing {
                what: "the wasm32-unknown-unknown target",
                fix: "rustup target add wasm32-unknown-unknown",
            });
        }
        missing
    }
}

/// A missing requirement and how to fix it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Missing {
    /// What is missing.
    pub what: &'static str,
    /// How to install it.
    pub fix: &'static str,
}

impl Probe {
    fn tool(&self, name: &str) -> Tool {
        Tool {
            name: name.to_string(),
            path: self.find(name),
            version: self
                .run(name, &["--version"])
                .and_then(|out| out.lines().next().map(str::to_string)),
        }
    }

    /// Runs `program args` and returns its trimmed stdout if it succeeded.
    fn run(&self, program: &str, args: &[&str]) -> Option<String> {
        self.output(&self.find(program)?, args).ok()
    }

    /// The `PATH` value in effect.
    pub fn path_var(&self) -> OsString {
        self.path
            .clone()
            .or_else(|| std::env::var_os("PATH"))
            .unwrap_or_default()
    }

    /// Finds an executable named `name` on the `PATH`.
    pub fn find(&self, name: &str) -> Option<PathBuf> {
        let names: Vec<String> = if cfg!(windows) {
            vec![
                format!("{name}.exe"),
                format!("{name}.cmd"),
                name.to_string(),
            ]
        } else {
            vec![name.to_string()]
        };
        std::env::split_paths(&self.path_var())
            .filter(|dir| !dir.as_os_str().is_empty())
            .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
            .find(|candidate| is_executable(candidate))
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

fn has_wasm_target(sysroot: &Path) -> bool {
    sysroot
        .join("lib/rustlib")
        .join(WASM_TARGET)
        .join("lib")
        .is_dir()
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

/// A program the builder runs: where it is and what it says its version is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// Its path.
    pub path: PathBuf,
    /// Its version output (`rustc -vV` gives several lines).
    pub version: String,
}

/// `rustc` plus what it says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rustc {
    /// The program; `version` is the full `rustc -vV` output, used in cache keys.
    pub program: Program,
    /// The `release:` line, e.g. `1.99.1`.
    pub release: String,
    /// The sysroot (`rustc --print sysroot`).
    pub sysroot: PathBuf,
}

impl Rustc {
    /// Whether the sysroot has the wasm32 standard library.
    pub fn has_wasm_target(&self) -> bool {
        has_wasm_target(&self.sysroot)
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
        program: Program { path, version },
        release,
        sysroot,
    })
}

/// Finds `cargo` and asks for its version.
pub fn cargo(probe: &Probe) -> Result<Program, String> {
    let path = probe
        .find("cargo")
        .ok_or_else(|| "`cargo` was not found on PATH".to_string())?;
    let version = probe.output(&path, &["--version"])?;
    Ok(Program { path, version })
}

/// Finds `wasm-opt` (Binaryen) and asks for its version.
pub fn wasm_opt(probe: &Probe) -> Option<Program> {
    let path = probe.find("wasm-opt")?;
    let version = probe.output(&path, &["--version"]).ok()?;
    Some(Program { path, version })
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
    fn an_empty_path_finds_nothing() {
        let dir = std::env::temp_dir();
        let probe = Probe {
            path: Some(OsString::new()),
            cwd: Some(dir),
        };
        let toolchain = Toolchain::detect_with(&probe);
        assert!(!toolchain.rustc.is_available());
        assert!(!toolchain.wasm32_target);
        assert!(!toolchain.can_build_islands());
        assert_eq!(toolchain.missing()[0].what, "rustc and cargo");
        assert_eq!(rustc(&probe).unwrap_err(), "`rustc` was not found on PATH");
        assert_eq!(cargo(&probe).unwrap_err(), "`cargo` was not found on PATH");
        assert!(wasm_opt(&probe).is_none());
    }

    #[test]
    fn the_workspace_toolchain_is_complete() {
        // The workspace pins a toolchain with the wasm32 target (rust-toolchain.toml).
        let probe = Probe {
            path: None,
            cwd: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
        };
        let toolchain = Toolchain::detect_with(&probe);
        assert!(toolchain.rustc.is_available(), "{toolchain:?}");
        assert!(toolchain.cargo.is_available(), "{toolchain:?}");
        assert!(toolchain.wasm32_target, "{toolchain:?}");
        assert!(toolchain.can_build_islands());
        let rustc = rustc(&probe).unwrap();
        assert!(rustc.has_wasm_target());
        assert!(rustc.program.version.contains("release: "));
    }
}
