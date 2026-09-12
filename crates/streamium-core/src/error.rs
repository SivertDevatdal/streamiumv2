use thiserror::Error;

/// Errors surfaced by the core. Kept deliberately small and stringly so that
/// they map 1:1 onto an FFI error enum without losing information.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CoreError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    #[error("playlist is not an M3U file (missing #EXTM3U header)")]
    NotAPlaylist,

    #[error("XMLTV parse error: {0}")]
    Xmltv(String),

    #[error("JSON decode error: {0}")]
    Json(String),

    #[error("decompression error: {0}")]
    Decompress(String),
}

impl From<serde_json::Error> for CoreError {
    fn from(e: serde_json::Error) -> Self {
        CoreError::Json(e.to_string())
    }
}

impl From<url::ParseError> for CoreError {
    fn from(e: url::ParseError) -> Self {
        CoreError::InvalidUrl(e.to_string())
    }
}
