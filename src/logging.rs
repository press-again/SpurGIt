//! Logging.
//!
//! Every diagnostic goes through [`log!`], which writes to stderr. Each line
//! carries seconds since startup.

use std::fmt;
use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

macro_rules! log {
    ($($arg:tt)*) => { $crate::logging::write(format_args!($($arg)*)) };
}
pub(crate) use log;

/// Start the run clock.
pub fn init() {
    let _ = START.set(Instant::now());
}

/// One log line: `[  12.345] message`; embedded newlines are split so every
/// physical line is prefixed.
pub fn write(args: fmt::Arguments<'_>) {
    let message = format!("{args}");
    let elapsed = START
        .get()
        .map(|start| start.elapsed().as_secs_f64())
        .unwrap_or(0.0);
    for line in message.lines() {
        eprintln!("spur: [{elapsed:>9.3}] {line}");
    }
}
