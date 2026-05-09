use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("regex error: {0}")]
    Regex(#[from] regex::Error),

    #[error("notify error: {0}")]
    Notify(#[from] notify::Error),

    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),

    /// Domain row と raw payload が一致せず復旧不能。手動介入が必要。
    #[error("fatal corruption: raw event id={0}: {1}")]
    FatalCorruption(i64, String),

    #[error("config error: {0}")]
    Config(String),

    #[error("operation cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;
