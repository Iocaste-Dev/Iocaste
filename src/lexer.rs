//! Lexer: source text -> tokens.
//!
//! Handles identifiers, keywords (as reserved identifiers), numbers (decimal, hex, binary,
//! floats, suffixes), strings, interpolated strings (`$"..{expr}.."`), chars, operators
//! and both comment styles.

use crate::diag::{Diagnostic, Span};

/// Words that cannot be used as names. (`get`, `set`, `where` are contextual, not reserved.)
pub const RESERVED: &[&str] = &[
    // declarations
    "namespace", "import", "class", "interface", "enum", "struct", "record",
    // modifiers
    "public", "private", "protected", "internal", "static", "abstract", "override", "virtual",
    "sealed", "readonly", "async", "partial", "unsafe", "constructor", "destructor",
    // statements and expressions
    "let", "var", "if", "else", "while", "for", "in", "return", "break", "continue", "new",
    "match", "spawn", "true", "false", "this", "base", "ref", "mut", "await",
    // things Iocaste deliberately does not have (rejected with a helpful message)
    "null", "throw", "try", "catch", "finally",
];

pub fn is_reserved(word: &str) -> bool {
    RESERVED.contains(&word)
}

#[derive(Debug, Clone, PartialEq)]
pub enum InterpPart {
    Text(String),
    /// Source text of a `{...}` hole, parsed later by the parser.
    Expr { src: String, span: Span },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Int { text: String, suffix: String },
    Float { text: String, suffix: String },
    Str(String),
    Interp(Vec<InterpPart>),
    Char(char),

    LParen, RParen, LBrace, RBrace, LBracket, RBracket,
    Comma, Semi, Colon, Dot, At,
    Plus, Minus, Star, Slash, Percent,
    Assign, PlusAssign, MinusAssign, StarAssign, SlashAssign, PercentAssign,
    EqEq, NotEq, Lt, Gt, LtEq, GtEq,
    AndAnd, OrOr, Bang, Question, QuestionQuestion,
    Arrow,
    PlusPlus, MinusMinus,
    Eof,
}

impl Tok {
    /// The source text of a punctuation token (empty for other tokens).
    pub fn symbol(&self) -> &'static str {
        match self {
            Tok::LParen => "(", Tok::RParen => ")", Tok::LBrace => "{", Tok::RBrace => "}",
            Tok::LBracket => "[", Tok::RBracket => "]",
            Tok::Comma => ",", Tok::Semi => ";", Tok::Colon => ":", Tok::Dot => ".", Tok::At => "@",
            Tok::Plus => "+", Tok::Minus => "-", Tok::Star => "*", Tok::Slash => "/", Tok::Percent => "%",
            Tok::Assign => "=", Tok::PlusAssign => "+=", Tok::MinusAssign => "-=",
            Tok::StarAssign => "*=", Tok::SlashAssign => "/=", Tok::PercentAssign => "%=",
            Tok::EqEq => "==", Tok::NotEq => "!=", Tok::Lt => "<", Tok::Gt => ">",
            Tok::LtEq => "<=", Tok::GtEq => ">=",
            Tok::AndAnd => "&&", Tok::OrOr => "||", Tok::Bang => "!",
            Tok::Question => "?", Tok::QuestionQuestion => "??",
            Tok::Arrow => "=>", Tok::PlusPlus => "++", Tok::MinusMinus => "--",
            _ => "",
        }
    }

    /// Human-readable description used in error messages.
    pub fn describe(&self) -> String {
        match self {
            Tok::Ident(s) if is_reserved(s) => format!("keyword `{}`", s),
            Tok::Ident(s) => format!("identifier `{}`", s),
            Tok::Int { text, suffix } | Tok::Float { text, suffix } => format!("number `{}{}`", text, suffix),
            Tok::Str(_) => "a string literal".into(),
            Tok::Interp(_) => "an interpolated string".into(),
            Tok::Char(_) => "a character literal".into(),
            Tok::Eof => "end of file".into(),
            other => format!("`{}`", other.symbol()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    /// Where the token starts.
    pub span: Span,
    /// Position just after the token (used to report "missing `;`" at the end of the line).
    pub end: Span,
}

/// Lex a whole source file. The result always ends with `Tok::Eof`.
pub fn lex(src: &str) -> Result<Vec<Token>, Diagnostic> {
    lex_at(src, 1, 1)
}

/// Like [`lex`], but positions start at the given line/column
/// (used for `{...}` holes inside interpolated strings).
pub fn lex_at(src: &str, line: u32, col: u32) -> Result<Vec<Token>, Diagnostic> {
    let chars: Vec<char> = src.chars().collect();
    let start = if chars.first() == Some(&'\u{feff}') { 1 } else { 0 }; // skip BOM
    let mut lx = Lexer { chars, pos: start, line, col };
    let mut out = Vec::new();
    loop {
        lx.skip_trivia()?;
        let span = lx.span();
        match lx.peek() {
            None => {
                out.push(Token { tok: Tok::Eof, span, end: span });
                break;
            }
            Some(c) => {
                let tok = lx.next_token(c, span)?;
                let end = lx.span();
                out.push(Token { tok, span, end });
            }
        }
    }
    Ok(out)
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: u32,
    col: u32,
}

impl Lexer {
    fn span(&self) -> Span {
        Span::new(self.line, self.col)
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn skip_trivia(&mut self) -> Result<(), Diagnostic> {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('/') if self.peek_at(1) == Some('/') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    let start = self.span();
                    self.bump();
                    self.bump();
                    loop {
                        match self.peek() {
                            None => return Err(Diagnostic::new("unterminated block comment", start)),
                            Some('*') if self.peek_at(1) == Some('/') => {
                                self.bump();
                                self.bump();
                                break;
                            }
                            _ => {
                                self.bump();
                            }
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn next_token(&mut self, c: char, span: Span) -> Result<Tok, Diagnostic> {
        if c.is_alphabetic() || c == '_' {
            let mut s = String::new();
            while let Some(ch) = self.peek() {
                if ch.is_alphanumeric() || ch == '_' {
                    s.push(ch);
                    self.bump();
                } else {
                    break;
                }
            }
            return Ok(Tok::Ident(s));
        }
        if c.is_ascii_digit() {
            return self.number(span);
        }
        match c {
            '"' => {
                self.bump();
                Ok(Tok::Str(self.string_body(span)?))
            }
            '$' => {
                if self.peek_at(1) == Some('"') {
                    self.bump();
                    self.bump();
                    self.interp_body(span)
                } else {
                    Err(Diagnostic::new("unexpected `$`", span)
                        .with_hint("interpolated strings are written $\"...{value}...\""))
                }
            }
            '\'' => {
                self.bump();
                let ch = match self.bump() {
                    None | Some('\n') => return Err(Diagnostic::new("unterminated character literal", span)),
                    Some('\'') => return Err(Diagnostic::new("empty character literal", span)),
                    Some('\\') => self.escape(span)?,
                    Some(ch) => ch,
                };
                if !self.eat('\'') {
                    return Err(Diagnostic::new("unterminated character literal", span)
                        .with_hint("character literals hold exactly one character; use \"...\" for strings"));
                }
                Ok(Tok::Char(ch))
            }
            _ => self.operator(c, span),
        }
    }

    fn operator(&mut self, c: char, span: Span) -> Result<Tok, Diagnostic> {
        self.bump();
        let tok = match c {
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            '{' => Tok::LBrace,
            '}' => Tok::RBrace,
            '[' => Tok::LBracket,
            ']' => Tok::RBracket,
            ',' => Tok::Comma,
            ';' => Tok::Semi,
            ':' => Tok::Colon,
            '.' => Tok::Dot,
            '@' => Tok::At,
            '=' => {
                if self.eat('=') { Tok::EqEq } else if self.eat('>') { Tok::Arrow } else { Tok::Assign }
            }
            '!' => if self.eat('=') { Tok::NotEq } else { Tok::Bang },
            '<' => if self.eat('=') { Tok::LtEq } else { Tok::Lt },
            // `>` is never merged into `>>` so that `List<List<int>>` just works.
            '>' => if self.eat('=') { Tok::GtEq } else { Tok::Gt },
            '&' => {
                if self.eat('&') {
                    Tok::AndAnd
                } else {
                    return Err(Diagnostic::new("unexpected `&`", span)
                        .with_hint("borrowing is written `ref` / `mut ref`; logical and is `&&`"));
                }
            }
            '|' => {
                if self.eat('|') {
                    Tok::OrOr
                } else {
                    return Err(Diagnostic::new("unexpected `|`", span).with_hint("logical or is `||`"));
                }
            }
            '?' => if self.eat('?') { Tok::QuestionQuestion } else { Tok::Question },
            '+' => {
                if self.eat('+') { Tok::PlusPlus } else if self.eat('=') { Tok::PlusAssign } else { Tok::Plus }
            }
            '-' => {
                if self.eat('-') { Tok::MinusMinus } else if self.eat('=') { Tok::MinusAssign } else { Tok::Minus }
            }
            '*' => if self.eat('=') { Tok::StarAssign } else { Tok::Star },
            '/' => if self.eat('=') { Tok::SlashAssign } else { Tok::Slash },
            '%' => if self.eat('=') { Tok::PercentAssign } else { Tok::Percent },
            other => {
                return Err(Diagnostic::new(format!("unexpected character `{}`", other), span));
            }
        };
        Ok(tok)
    }

    fn digits(&mut self, text: &mut String) {
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                text.push(c);
                self.bump();
            } else if c == '_' {
                self.bump();
            } else {
                break;
            }
        }
    }

    fn number(&mut self, span: Span) -> Result<Tok, Diagnostic> {
        let mut text = String::new();
        let mut is_float = false;

        if self.peek() == Some('0') && matches!(self.peek_at(1), Some('x' | 'X' | 'b' | 'B')) {
            self.bump();
            let radix = self.bump().unwrap().to_ascii_lowercase();
            text.push('0');
            text.push(radix);
            let mut count = 0;
            while let Some(c) = self.peek() {
                let ok = if radix == 'x' { c.is_ascii_hexdigit() } else { c == '0' || c == '1' };
                if ok {
                    text.push(c);
                    count += 1;
                    self.bump();
                } else if c == '_' {
                    self.bump();
                } else {
                    break;
                }
            }
            if count == 0 {
                return Err(Diagnostic::new("expected digits after the radix prefix", span));
            }
        } else {
            self.digits(&mut text);
            // a fraction needs a digit after the dot, so `1.ToString()` still works
            if self.peek() == Some('.') && self.peek_at(1).map_or(false, |c| c.is_ascii_digit()) {
                is_float = true;
                text.push('.');
                self.bump();
                self.digits(&mut text);
            }
            if matches!(self.peek(), Some('e' | 'E')) {
                let has_exp = match self.peek_at(1) {
                    Some(c) if c.is_ascii_digit() => true,
                    Some('+' | '-') => self.peek_at(2).map_or(false, |c| c.is_ascii_digit()),
                    _ => false,
                };
                if has_exp {
                    is_float = true;
                    self.bump();
                    text.push('e');
                    if matches!(self.peek(), Some('+' | '-')) {
                        text.push(self.bump().unwrap());
                    }
                    self.digits(&mut text);
                }
            }
        }

        let mut suffix = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                suffix.push(c);
                self.bump();
            } else {
                break;
            }
        }
        let lower = suffix.to_ascii_lowercase();
        if !["", "f", "d", "l", "u", "ul", "lu"].contains(&lower.as_str()) {
            return Err(Diagnostic::new(format!("invalid numeric suffix `{}`", suffix), span)
                .with_hint("valid suffixes: f, d (floating point), l, u, ul (integers)"));
        }
        let float_suffix = lower == "f" || lower == "d";
        if is_float && !(lower.is_empty() || float_suffix) {
            return Err(Diagnostic::new(format!("suffix `{}` cannot be used on a floating-point number", suffix), span));
        }
        if is_float || float_suffix {
            Ok(Tok::Float { text, suffix })
        } else {
            Ok(Tok::Int { text, suffix })
        }
    }

    fn escape(&mut self, start: Span) -> Result<char, Diagnostic> {
        let esc_span = self.span();
        match self.bump() {
            Some('n') => Ok('\n'),
            Some('t') => Ok('\t'),
            Some('r') => Ok('\r'),
            Some('0') => Ok('\0'),
            Some('\\') => Ok('\\'),
            Some('"') => Ok('"'),
            Some('\'') => Ok('\''),
            Some('u') => {
                if !self.eat('{') {
                    return Err(Diagnostic::new("expected `{` after `\\u`", esc_span).with_hint("unicode escapes look like \\u{1F600}"));
                }
                let mut hex = String::new();
                while let Some(c) = self.peek() {
                    if c.is_ascii_hexdigit() {
                        hex.push(c);
                        self.bump();
                    } else {
                        break;
                    }
                }
                if !self.eat('}') || hex.is_empty() || hex.len() > 6 {
                    return Err(Diagnostic::new("malformed unicode escape", esc_span).with_hint("unicode escapes look like \\u{1F600}"));
                }
                u32::from_str_radix(&hex, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| Diagnostic::new("invalid unicode code point", esc_span))
            }
            Some(c) => Err(Diagnostic::new(format!("unknown escape sequence `\\{}`", c), esc_span)),
            None => Err(Diagnostic::new("unterminated string literal", start)),
        }
    }

    fn string_body(&mut self, start: Span) -> Result<String, Diagnostic> {
        let mut s = String::new();
        loop {
            match self.bump() {
                None | Some('\n') => return Err(Diagnostic::new("unterminated string literal", start)),
                Some('"') => return Ok(s),
                Some('\\') => s.push(self.escape(start)?),
                Some(c) => s.push(c),
            }
        }
    }

    fn interp_body(&mut self, start: Span) -> Result<Tok, Diagnostic> {
        let unterminated = || Diagnostic::new("unterminated interpolated string", start);
        let mut parts = Vec::new();
        let mut text = String::new();
        loop {
            match self.peek() {
                None | Some('\n') => return Err(unterminated()),
                Some('"') => {
                    self.bump();
                    break;
                }
                Some('\\') => {
                    self.bump();
                    text.push(self.escape(start)?);
                }
                Some('{') => {
                    if self.peek_at(1) == Some('{') {
                        self.bump();
                        self.bump();
                        text.push('{');
                        continue;
                    }
                    self.bump();
                    if !text.is_empty() {
                        parts.push(InterpPart::Text(std::mem::take(&mut text)));
                    }
                    let espan = self.span();
                    let mut src = String::new();
                    let mut depth = 1;
                    loop {
                        match self.bump() {
                            None | Some('\n') => {
                                return Err(Diagnostic::new("unterminated `{` in interpolated string", espan));
                            }
                            Some('{') => {
                                depth += 1;
                                src.push('{');
                            }
                            Some('}') => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                                src.push('}');
                            }
                            Some('"') => {
                                // a string literal nested inside the hole: copy it verbatim
                                src.push('"');
                                loop {
                                    match self.bump() {
                                        None | Some('\n') => return Err(unterminated()),
                                        Some('\\') => {
                                            src.push('\\');
                                            if let Some(n) = self.bump() {
                                                src.push(n);
                                            }
                                        }
                                        Some('"') => {
                                            src.push('"');
                                            break;
                                        }
                                        Some(c) => src.push(c),
                                    }
                                }
                            }
                            Some(c) => src.push(c),
                        }
                    }
                    if src.trim().is_empty() {
                        return Err(Diagnostic::new("empty `{}` in interpolated string", espan)
                            .with_hint("write `{{` and `}}` for literal braces"));
                    }
                    parts.push(InterpPart::Expr { src, span: espan });
                }
                Some('}') => {
                    if self.peek_at(1) == Some('}') {
                        self.bump();
                        self.bump();
                        text.push('}');
                    } else {
                        return Err(Diagnostic::new("single `}` in interpolated string", self.span())
                            .with_hint("write `}}` for a literal brace"));
                    }
                }
                Some(c) => {
                    self.bump();
                    text.push(c);
                }
            }
        }
        if !text.is_empty() {
            parts.push(InterpPart::Text(text));
        }
        Ok(Tok::Interp(parts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex(s).unwrap().into_iter().map(|t| t.tok).collect()
    }

    fn int(t: &str, s: &str) -> Tok {
        Tok::Int { text: t.into(), suffix: s.into() }
    }

    fn float(t: &str, s: &str) -> Tok {
        Tok::Float { text: t.into(), suffix: s.into() }
    }

    #[test]
    fn numbers() {
        assert_eq!(
            toks("1_000 0xFF 0b101 1.5 2e3 3f 7u"),
            vec![int("1000", ""), int("0xFF", ""), int("0b101", ""), float("1.5", ""), float("2e3", ""), float("3", "f"), int("7", "u"), Tok::Eof]
        );
    }

    #[test]
    fn member_access_on_number_is_not_a_float() {
        assert_eq!(toks("1.x"), vec![int("1", ""), Tok::Dot, Tok::Ident("x".into()), Tok::Eof]);
    }

    #[test]
    fn operators_and_generics_close() {
        assert_eq!(
            toks("a ?? b => c >= d > > e"),
            vec![
                Tok::Ident("a".into()), Tok::QuestionQuestion, Tok::Ident("b".into()), Tok::Arrow,
                Tok::Ident("c".into()), Tok::GtEq, Tok::Ident("d".into()), Tok::Gt, Tok::Gt,
                Tok::Ident("e".into()), Tok::Eof
            ]
        );
    }

    #[test]
    fn comments_are_skipped() {
        assert_eq!(toks("a // x\n /* y */ b"), vec![Tok::Ident("a".into()), Tok::Ident("b".into()), Tok::Eof]);
    }

    #[test]
    fn interpolation_parts() {
        match &toks("$\"a {x} b {{ok}}\"")[0] {
            Tok::Interp(parts) => {
                assert_eq!(parts.len(), 3);
                assert_eq!(parts[2], InterpPart::Text(" b {ok}".into()));
            }
            t => panic!("unexpected {:?}", t),
        }
    }

    #[test]
    fn positions_are_tracked() {
        let t = lex("a\n  b").unwrap();
        assert_eq!(t[1].span, Span::new(2, 3));
    }

    #[test]
    fn errors() {
        assert!(lex("\"abc").is_err());
        assert!(lex("/* abc").is_err());
        assert!(lex("'ab'").is_err());
        assert!(lex("12abc").is_err());
        assert!(lex("a & b").is_err());
        assert!(lex("\"\\q\"").is_err());
    }
}
