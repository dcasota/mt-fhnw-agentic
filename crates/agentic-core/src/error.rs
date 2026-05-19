//! Crate-wide error type.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("serde_json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),

    #[error("blob not found: {0}")]
    BlobNotFound(String),

    #[error("tree not found: {0}")]
    TreeNotFound(String),

    #[error("commit not found: {0}")]
    CommitNotFound(String),

    #[error("ref not found: {0}")]
    RefNotFound(String),

    #[error("project not found: {0}")]
    ProjectNotFound(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("schema migration failed at version {version}: {source}")]
    Migration {
        version: u32,
        #[source]
        source: rusqlite::Error,
    },

    #[error("unsupported schema version {0} (newest known: {1})")]
    UnsupportedSchema(u32, u32),

    #[error("encoding error: {0}")]
    Encoding(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
