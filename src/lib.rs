//! Iocaste language front-end (v0.0.2-alpha): lexer, parser and AST.
//!
//! Pipeline so far: source text -> [`lexer`] -> tokens -> [`parser`] -> [`ast`].
//! Later versions add name resolution, type checking, borrow checking and backends.

pub mod ast;
pub mod diag;
pub mod dump;
pub mod lexer;
pub mod parser;

/// Version of this toolchain (also the language version for now).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
