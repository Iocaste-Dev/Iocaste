//! Diagnostics: source positions and human-friendly error rendering.

use std::fmt;

/// A 1-based line/column position in a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub col: u32,
}

impl Span {
    pub fn new(line: u32, col: u32) -> Self {
        Span { line, col }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// A compile error with a position and an optional hint.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub message: String,
    pub span: Span,
    pub hint: Option<String>,
}

impl Diagnostic {
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        Diagnostic { message: message.into(), span, hint: None }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Render the error with the offending source line and a caret.
    ///
    /// ```text
    /// error: expected `;`, found `}`
    ///  --> src/Program.ioc:12:20
    ///    |
    /// 12 |     x: int }
    ///    |            ^
    ///    = help: ...
    /// ```
    pub fn render(&self, file: &str, source: &str) -> String {
        let line_no = self.span.line as usize;
        let w = line_no.to_string().len();
        let mut out = String::new();
        out.push_str(&format!("error: {}\n", self.message));
        out.push_str(&format!("{:>pad$}--> {}:{}:{}\n", "", file, self.span.line, self.span.col, pad = w));
        out.push_str(&format!("{:>w$} |\n", "", w = w));
        if let Some(text) = source.lines().nth(line_no.saturating_sub(1)) {
            out.push_str(&format!("{:>w$} | {}\n", line_no, text, w = w));
            let pad: String = text
                .chars()
                .take((self.span.col as usize).saturating_sub(1))
                .map(|c| if c == '\t' { '\t' } else { ' ' })
                .collect();
            out.push_str(&format!("{:>w$} | {}^\n", "", pad, w = w));
        }
        if let Some(h) = &self.hint {
            out.push_str(&format!("{:>w$} = help: {}\n", "", h, w = w));
        }
        out
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.span, self.message)
    }
}

impl std::error::Error for Diagnostic {}
