use std::path::PathBuf;

/// Errors returned by the core.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("TermRoost is already running (lock held on {0})")]
    AlreadyRunning(PathBuf),
    #[error(
        "agentdeck (TermRoost's old name) is still running and using {0}. Close it, then start TermRoost again \
         to move your projects over."
    )]
    LegacyRunning(PathBuf),
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("{0}")]
    Invalid(String),
    #[error("could not determine a data directory; set TERMROOST_DATA_DIR")]
    NoDataDir,
}

pub type Result<T> = std::result::Result<T, Error>;
