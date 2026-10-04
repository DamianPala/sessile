//! Ctrl-C. A read stops at once with an `interrupted` error. A change hears
//! of it between two of its steps, so it can stop cleanly and say what it
//! already did instead of dying halfway through a move or a delete.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{EXIT_INTERRUPTED, Kind, Result, SessileError};

static STRUCTURED: AtomicBool = AtomicBool::new(false);
static CHANGING: AtomicBool = AtomicBool::new(false);
static REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn install(structured: bool) {
    STRUCTURED.store(structured, Ordering::SeqCst);
    // Without a handler the default action still ends the process; only the
    // error object is lost.
    let _ = ctrlc::set_handler(|| {
        if CHANGING.load(Ordering::SeqCst) {
            REQUESTED.store(true, Ordering::SeqCst);
            return;
        }
        crate::cli::report_error(&interrupted(), STRUCTURED.load(Ordering::SeqCst));
        std::process::exit(i32::from(EXIT_INTERRUPTED));
    });
}

pub fn interrupted() -> SessileError {
    SessileError::new(Kind::Interrupted, "interrupted by Ctrl-C")
}

/// Held while state changes; Ctrl-C then only sets a flag that `check` reads.
pub struct Changing;

impl Changing {
    pub fn begin() -> Self {
        CHANGING.store(true, Ordering::SeqCst);
        Self
    }
}

impl Drop for Changing {
    fn drop(&mut self) {
        CHANGING.store(false, Ordering::SeqCst);
    }
}

/// Fails with `interrupted` once Ctrl-C was pressed during a change.
pub fn check() -> Result<()> {
    if REQUESTED.load(Ordering::SeqCst) {
        Err(interrupted())
    } else {
        Ok(())
    }
}
