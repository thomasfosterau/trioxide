//! Detecting the Rust toolchain that island builds need (PLAN D6).
//!
//! [`Toolchain::detect`] probes `rustc`, `cargo`, the `wasm32-unknown-unknown` target and
//! `wasm-opt`. `trioxide check-toolchain` prints the result; the island builder uses the
//! same probe to decide whether to fall back to JS.

use std::{
    ffi::OsString,
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
            .then(|| probe.output("rustc", &["--print", "sysroot"]))
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
            path: self.which(name),
            version: self
                .output(name, &["--version"])
                .and_then(|out| out.lines().next().map(str::to_string)),
        }
    }

    /// Runs `program args` and returns its trimmed stdout if it succeeded.
    fn output(&self, program: &str, args: &[&str]) -> Option<String> {
        let program = self.which(program)?;
        let mut command = Command::new(program);
        command.args(args);
        if let Some(path) = &self.path {
            command.env("PATH", path);
        }
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        let output = command.output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn which(&self, name: &str) -> Option<PathBuf> {
        let path = self.path.clone().or_else(|| std::env::var_os("PATH"))?;
        let names: Vec<String> = if cfg!(windows) {
            vec![
                format!("{name}.exe"),
                format!("{name}.cmd"),
                name.to_string(),
            ]
        } else {
            vec![name.to_string()]
        };
        std::env::split_paths(&path)
            .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
            .find(|candidate| is_executable(candidate))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_path_finds_nothing() {
        let dir = std::env::temp_dir();
        let toolchain = Toolchain::detect_with(&Probe {
            path: Some(OsString::new()),
            cwd: Some(dir),
        });
        assert!(!toolchain.rustc.is_available());
        assert!(!toolchain.wasm32_target);
        assert!(!toolchain.can_build_islands());
        assert_eq!(toolchain.missing()[0].what, "rustc and cargo");
    }

    #[test]
    fn the_workspace_toolchain_is_complete() {
        // The workspace pins a toolchain with the wasm32 target (rust-toolchain.toml).
        let toolchain = Toolchain::detect_with(&Probe {
            path: None,
            cwd: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
        });
        assert!(toolchain.rustc.is_available(), "{toolchain:?}");
        assert!(toolchain.cargo.is_available(), "{toolchain:?}");
        assert!(toolchain.wasm32_target, "{toolchain:?}");
        assert!(toolchain.can_build_islands());
    }
}
