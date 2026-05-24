//! Error type shared across `klippo-core`.

/// Errors produced by the core library.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("config parse error: {0}")]
    ConfigParse(#[from] toml::de::Error),

    #[error("config serialize error: {0}")]
    ConfigSer(#[from] toml::ser::Error),

    #[error("invalid regex: {0}")]
    Regex(#[from] regex::Error),

    #[error("invalid action command: {0}")]
    ActionParse(String),
}

/// Convenience result alias for the core library.
pub type Result<T> = std::result::Result<T, Error>;
