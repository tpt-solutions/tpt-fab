//! Core error type for `tpt-fab`.

use std::fmt;

/// Error type for the `tpt-fab` core stack.
#[derive(Debug)]
pub enum FabError {
    /// SECS-II encode/decode failure.
    SecsDecode(String),
    /// HSMS framing or protocol failure.
    Hsms(String),
    /// I/O failure on the transport.
    Io(std::io::Error),
    /// GEM state-model violation (e.g. message not allowed in current state).
    Gem(String),
    /// GEM300 object/job error.
    Gem300(String),
    /// Recipe management error (unknown version, drift hard-fail, etc.).
    Recipe(String),
    /// Lot tracking error.
    Lot(String),
    /// Timeout waiting for a reply (maps to HSMS T3/T6 semantics by call site).
    Timeout(String),
    /// The peer or this side is not in the required state.
    NotSelected,
    /// Connection already closed.
    Disconnected,
}

impl fmt::Display for FabError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FabError::SecsDecode(s) => write!(f, "SECS-II decode error: {s}"),
            FabError::Hsms(s) => write!(f, "HSMS error: {s}"),
            FabError::Io(e) => write!(f, "I/O error: {e}"),
            FabError::Gem(s) => write!(f, "GEM error: {s}"),
            FabError::Gem300(s) => write!(f, "GEM300 error: {s}"),
            FabError::Recipe(s) => write!(f, "recipe error: {s}"),
            FabError::Lot(s) => write!(f, "lot error: {s}"),
            FabError::Timeout(s) => write!(f, "timeout: {s}"),
            FabError::NotSelected => write!(f, "HSMS session not selected"),
            FabError::Disconnected => write!(f, "connection closed"),
        }
    }
}

impl std::error::Error for FabError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FabError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for FabError {
    fn from(e: std::io::Error) -> Self {
        FabError::Io(e)
    }
}
