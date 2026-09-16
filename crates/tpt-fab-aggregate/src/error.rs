//! Errors for the outcome-file exchange and aggregation.

use std::fmt;

/// Error type for `tpt-fab-aggregate`.
#[derive(Debug)]
pub enum AggregateError {
    /// Malformed JSON in an outcome file.
    Json(String),
    /// The file parsed but violates the schema (wrong types, missing fields).
    Schema(String),
    /// The report's schema version is not understood (unknown minor/major).
    UnsupportedSchemaVersion { found: String },
    /// Signature missing, malformed, or not matching the file contents.
    Signature(String),
    /// I/O failure while writing or reading a file.
    Io(std::io::Error),
    /// Invalid input to an aggregation operation.
    InvalidInput(String),
}

impl fmt::Display for AggregateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AggregateError::Json(s) => write!(f, "JSON error: {s}"),
            AggregateError::Schema(s) => write!(f, "schema error: {s}"),
            AggregateError::UnsupportedSchemaVersion { found } => {
                write!(f, "unsupported schema version {found} — refusing to guess field meanings")
            }
            AggregateError::Signature(s) => write!(f, "signature error: {s}"),
            AggregateError::Io(e) => write!(f, "I/O error: {e}"),
            AggregateError::InvalidInput(s) => write!(f, "invalid input: {s}"),
        }
    }
}

impl std::error::Error for AggregateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AggregateError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for AggregateError {
    fn from(e: std::io::Error) -> Self {
        AggregateError::Io(e)
    }
}
