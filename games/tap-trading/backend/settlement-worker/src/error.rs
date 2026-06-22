use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),

    // Boxed to keep WorkerError below clippy::result_large_err threshold.
    // `From<tungstenite::Error>` is implemented manually below so `?` still works.
    #[error("websocket: {0}")]
    Ws(Box<tokio_tungstenite::tungstenite::Error>),

    #[error("decode oracle message: {0}")]
    Decode(#[from] serde_json::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("missing env var: {0}")]
    MissingEnv(&'static str),

    #[error("invalid env var {name}: {reason}")]
    InvalidEnv { name: &'static str, reason: String },

    // f64 value (oracle mid, multiplier) not representable as NUMERIC — NaN or
    // ±Inf. Surfacing rather than panicking lets the loop log + retry without
    // killing the spawned task; a bad tick.mid pins the position until a clean
    // tick arrives, which is the desired behavior (idempotent via UNIQUE gate).
    #[error("non-finite numeric for {context}: {value}")]
    NonFiniteNumeric { context: &'static str, value: String },
}

pub type Result<T> = std::result::Result<T, WorkerError>;

impl From<tokio_tungstenite::tungstenite::Error> for WorkerError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        WorkerError::Ws(Box::new(e))
    }
}
