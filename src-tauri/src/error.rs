use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Mongo(#[from] mongodb::error::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("secret store error: {0}")]
    Secret(String),
    #[error("ssh tunnel error: {0}")]
    Ssh(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// The data changed under an edit, or the edit would clobber something:
    /// the message says what to do, so it carries no prefix.
    #[error("{0}")]
    Conflict(String),
    #[error("session {0} is not connected")]
    SessionNotFound(String),
    /// Stopped by the user before it finished.
    #[error("cancelled")]
    Cancelled,
    /// A connection that logs in with a password has none to send. Shown
    /// as it is, so the message carries no prefix.
    #[error("{0}")]
    PasswordMissing(String),
    /// Assistant failures are shown to the user as they are, so the message
    /// carries no prefix.
    #[error("{0}")]
    Assistant(String),
}

// Tauri serializes command errors as JSON for the frontend; a plain string
// keeps the invoke() reject reason simple to consume there.
impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
