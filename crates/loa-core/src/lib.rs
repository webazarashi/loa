//! loa-core: scenario engine, agent-browser driver, LINE API client, keychain,
//! approval policy and diffing for the `loa` CLI.

// `LoaError` carries hint/next/report for the user; errors end a command, so their size
// does not matter for speed.
#![allow(clippy::result_large_err)]

#[cfg(all(feature = "test-hooks", not(debug_assertions)))]
compile_error!("feature `test-hooks` is for debug test builds only");

pub mod actions;
pub mod browser;
pub mod bundle;
pub mod config;
pub mod domains;
pub mod error;
pub mod gate;
pub mod line_api;
pub mod ops;
pub mod output;
pub mod paths;
pub mod redact;
pub mod report;
pub mod resources;
pub mod scenario;
pub mod secrets;
pub mod task;
pub mod terms;
pub mod util;

pub use error::{Code, LoaError, Result};
pub use output::{Envelope, Outcome};
