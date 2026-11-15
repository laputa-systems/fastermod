//! fastermod: stateless regex substitution and syntax-aware refactoring.
//!
//! Every invocation discovers and analyzes its inputs anew; there is no
//! daemon, persistent index, or language-server subprocess. See
//! docs/CAMPAIGN-STATELESS-REFACTOR.md and docs/refactor/cli.md.

// Engine surfaces land incrementally across the refactor campaign; the
// allow matches the previous include!-based layout.
#![allow(dead_code)]

pub(crate) type Error = Box<dyn std::error::Error + Send + Sync>;
pub(crate) type Result<T> = ::std::result::Result<T, Error>;

/// Process exit carrying the frozen exit-code contract (docs/refactor/cli.md).
#[derive(Debug)]
pub struct Failure {
    pub code: i32,
    pub message: String,
}

impl Failure {
    pub(crate) fn usage(message: impl Into<String>) -> Failure {
        Failure { code: 2, message: message.into() }
    }
    pub(crate) fn analysis(message: impl Into<String>) -> Failure {
        Failure { code: 3, message: message.into() }
    }
    pub(crate) fn io(message: impl Into<String>) -> Failure {
        Failure { code: 4, message: message.into() }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

pub(crate) fn operational<T>(result: Result<T>) -> std::result::Result<T, Failure> {
    result.map_err(|e| Failure::io(e.to_string()))
}

pub(crate) fn error_with_context(msg: impl std::fmt::Display, e: impl std::fmt::Display) -> Error {
    format!("{msg}: {e}").into()
}

macro_rules! ensure {
    ($cond:expr, $msg:expr $(,)?) => {
        if !$cond {
            return Err($msg.into());
        }
    };
}

mod bind;
mod bind_go;
mod bind_js;
mod bind_lua;
mod bind_py;
mod bind_rs;
mod cli;
mod diff;
mod discovery;
mod engine;
mod lang;
mod output;
mod plan;
mod rename;
mod review;
mod source;
mod syntax;
mod terminal;
mod text;
mod workspace;

#[doc(hidden)]
pub mod benchmark;

/// CLI entry point: runs the parsed request and returns the process exit code.
pub fn run() -> i32 {
    match engine::fastermod() {
        Ok(code) => code,
        Err(failure) => {
            eprintln!("error: {}", failure.message);
            failure.code
        }
    }
}
