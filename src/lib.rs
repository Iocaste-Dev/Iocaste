//! Iocaste language front-end (v0.0.3-alpha): lexer, parser, AST, name resolution, type checker.
//!
//! Pipeline so far:
//!   source text -> [`lexer`] -> tokens -> [`parser`] -> [`ast`]
//!   ASTs -> [`sema`] (symbols + [`check`]) -> errors
//! Later versions add the borrow checker and backends.

pub mod ast;
pub mod check;
pub mod diag;
pub mod dump;
pub mod lexer;
pub mod parser;
pub mod sema;
pub mod symbols;
pub mod ty;

/// Version of this toolchain (also the language version for now).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
