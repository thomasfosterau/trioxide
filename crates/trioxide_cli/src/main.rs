//! The `trioxide` command-line tool. The implementation is the `trioxide_cli` library, which
//! the npm package's bin also calls through the napi binding.

use std::process::ExitCode;

fn main() -> ExitCode {
    ExitCode::from(trioxide_cli::run(std::env::args_os()))
}
