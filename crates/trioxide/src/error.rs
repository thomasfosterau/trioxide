use std::path::PathBuf;

use trioxide_project::ProjectError;

/// Errors that stop a trioxide operation.
///
/// Problems in the user's code are not errors at this level: they are
/// [`Diagnostic`](crate::Diagnostic)s on the output, so one bad module doesn't hide the rest.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file could not be read or written.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A config file is invalid.
    #[error("invalid config {}: {message}", path.display())]
    Config {
        /// The config file.
        path: PathBuf,
        /// What is wrong with it.
        message: String,
    },
    /// Loading the project (entries, tsconfig) failed.
    #[error(transparent)]
    Project(#[from] ProjectError),
    /// A feature that is planned but not built yet.
    #[error("{0} is not yet implemented")]
    NotImplemented(&'static str),
}

/// `Result` with trioxide's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
