//! Semantic analysis entry point: name resolution + type checking of a set of parsed files.

use crate::ast::SourceFile;
use crate::check;
use crate::parser;
use crate::symbols::{Program, SemaError};

/// The built-in types (Option, Result, Console, List, ...), written in Iocaste itself.
pub const PRELUDE: &str = include_str!("prelude.ioc");

/// Analyze `files` together (they can refer to each other's types).
///
/// Returned errors carry the index of their file in `files`. An error whose file index is
/// `files.len()` belongs to the built-in prelude and indicates a bug in the compiler.
pub fn analyze(files: &[&SourceFile]) -> Vec<SemaError> {
    let prelude = parser::parse(PRELUDE).expect("the built-in prelude must parse");
    let prelude_index = files.len();
    let mut prog = Program::new(prelude_index);
    for (i, f) in files.iter().enumerate() {
        prog.add_file(i, f);
    }
    prog.add_file(prelude_index, &prelude);
    prog.finish();

    let mut errors = std::mem::take(&mut prog.errors);
    errors.extend(check::check_bodies(&prog));
    errors.sort_by_key(|e| (e.file, e.diag.span.line, e.diag.span.col));
    errors
}
