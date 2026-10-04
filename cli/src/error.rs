//! Error type shared by every command: a stable `kind`, a message, an optional
//! recovery hint and machine-readable context. Exit codes: 1 failure, 2 usage,
//! 130 interrupted.

use std::fmt;
use std::io;
use std::path::Path;

use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A usage error: unknown flag, bad value, malformed id.
    InvalidInput,
    /// The session id is unknown in the place it was looked up.
    NotFound,
    /// A destination path already exists; nothing was moved.
    Conflict,
    /// An irreversible change was asked for without `--yes`.
    ConfirmationRequired,
    /// The session is the current one or a Claude Code process runs it.
    SessionLive,
    /// `sessile/pins.json` exists but does not parse.
    PinsUnreadable,
    /// A file operation failed.
    IoError,
    /// A change failed part way and nothing tells whether it took effect.
    OutcomeUnknown,
    /// Ctrl-C stopped the command.
    Interrupted,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::InvalidInput,
        Kind::NotFound,
        Kind::Conflict,
        Kind::ConfirmationRequired,
        Kind::SessionLive,
        Kind::PinsUnreadable,
        Kind::IoError,
        Kind::OutcomeUnknown,
        Kind::Interrupted,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::InvalidInput => "invalid_input",
            Kind::NotFound => "not_found",
            Kind::Conflict => "conflict",
            Kind::ConfirmationRequired => "confirmation_required",
            Kind::SessionLive => "session_live",
            Kind::PinsUnreadable => "pins_unreadable",
            Kind::IoError => "io_error",
            Kind::OutcomeUnknown => "outcome_unknown",
            Kind::Interrupted => "interrupted",
        }
    }
}

pub const EXIT_FAILURE: u8 = 1;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_INTERRUPTED: u8 = 130;

#[derive(Debug)]
pub struct SessileError {
    pub kind: Kind,
    pub message: String,
    pub hint: Option<String>,
    pub context: Map<String, Value>,
}

pub type Result<T> = std::result::Result<T, SessileError>;

impl SessileError {
    pub fn new(kind: Kind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            hint: None,
            context: Map::new(),
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(Kind::InvalidInput, message)
    }

    pub fn io(op: &str, path: &Path, err: &io::Error) -> Self {
        Self::new(
            Kind::IoError,
            format!("{op} {} failed: {err}", path.display()),
        )
        .with_context("path", json!(path.display().to_string()))
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_context(mut self, key: &str, value: Value) -> Self {
        self.context.insert(key.to_string(), value);
        self
    }

    pub fn exit_code(&self) -> u8 {
        match self.kind {
            Kind::InvalidInput => EXIT_USAGE,
            Kind::Interrupted => EXIT_INTERRUPTED,
            _ => EXIT_FAILURE,
        }
    }

    /// The F3 envelope: `{"error":{"kind","message","hint"?,"context"?}}`.
    pub fn to_json(&self) -> Value {
        let mut error = Map::new();
        error.insert("kind".into(), json!(self.kind.name()));
        error.insert("message".into(), json!(self.message));
        if let Some(hint) = &self.hint {
            error.insert("hint".into(), json!(hint));
        }
        if !self.context.is_empty() {
            error.insert("context".into(), Value::Object(self.context.clone()));
        }
        json!({ "error": error })
    }
}

impl fmt::Display for SessileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SessileError {}
