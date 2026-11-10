//! A small in-process log.
//!
//! The app has no console when launched as a bundle, so debugging used to mean rebuilding with
//! temporary `eprintln!` calls. This keeps the last few hundred lines in memory for the About
//! diagnostics export, and mirrors them to stderr when `LUMEN_TRACE` is set.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const CAPACITY: usize = 500;

static LOG: Mutex<Option<VecDeque<String>>> = Mutex::new(None);
static START: OnceLock<Instant> = OnceLock::new();

fn record(level: &str, message: &str) {
    let start = START.get_or_init(Instant::now);
    let line = format!(
        "[{:>8}ms] {:<5} {}",
        start.elapsed().as_millis(),
        level,
        message
    );
    if std::env::var_os("LUMEN_TRACE").is_some() {
        eprintln!("{line}");
    }
    let mut slot = match LOG.lock() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    };
    let buffer = slot.get_or_insert_with(|| VecDeque::with_capacity(CAPACITY));
    if buffer.len() == CAPACITY {
        buffer.pop_front();
    }
    buffer.push_back(line);
}

pub fn trace(message: impl AsRef<str>) {
    record("trace", message.as_ref());
}

pub fn info(message: impl AsRef<str>) {
    record("info", message.as_ref());
}

pub fn warn(message: impl AsRef<str>) {
    record("warn", message.as_ref());
}

pub fn error(message: impl AsRef<str>) {
    record("error", message.as_ref());
}

/// Newest last, ready for a diagnostics paste.
pub fn snapshot() -> String {
    let slot = match LOG.lock() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    };
    match slot.as_ref() {
        Some(buffer) => buffer.iter().cloned().collect::<Vec<_>>().join("\n"),
        None => String::new(),
    }
}
