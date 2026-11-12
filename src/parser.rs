//! Recursive-descent parser: tokens -> AST.
//!
//! v0.0.2 stops at the first syntax error (error recovery comes later).

use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::lexer::{self, is_reserved, InterpPart, Tok, Token};

pub type PResult<T> = Result<T, Diagnostic>;

/// Parse a complete source file.
pub fn parse(src: &str) -> PResult<SourceFile> {
    let toks = lexer::lex(src)?;
    let mut p = Parser::new(toks);
    let items = p.parse_items(false)?;
    Ok(SourceFile { items })
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(toks: Vec<Token>) -> Self {
        Parser { toks, pos: 0 }
    }

    // ------------------------------------------------------------ helpers

    fn last(&self) -> usize {
        self.toks.len() - 1
    }

    fn cur(&self) -> &Token {
        &self.toks[self.pos.min(self.last())]
    }

    fn peek(&self) -> &Tok {
        &self.cur().tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.last())].tok
    }

    fn span(&self) -> Span {
        self.cur().span
    }

    fn bump(&mut self) -> Token {
        let t = self.cur().clone();
        if self.pos < self.last() {
            self.pos += 1;
        }
        t
    }

    fn at(&self, t: &Tok) -> bool {
        self.peek() == t
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.at(t) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn unexpected(&self, expected: &str) -> Diagnostic {
        Diagnostic::new(format!("expected {}, found {}", expected, self.peek().describe()), self.span())
    }

    /// Position just after the previous token.
    fn prev_end(&self) -> Option<Span> {
        if self.pos > 0 {
            Some(self.toks[(self.pos - 1).min(self.last())].end)
        } else {
            None
        }
    }

    fn expect(&mut self, t: &Tok) -> PResult<Span> {
        if self.at(t) {
            return Ok(self.bump().span);
        }
        let mut d = self.unexpected(&format!("`{}`", t.symbol()));
        // A missing `;` `)` `]` is best reported where it should have been: right after
        // the previous token, not at whatever token happens to follow.
        if matches!(t, Tok::Semi | Tok::RParen | Tok::RBracket) {
            if let Some(end) = self.prev_end() {
                d.span = end;
            }
        }
        Err(d)
    }

    /// The current word if the current token is an identifier or keyword.
    fn kw(&self) -> Option<&str> {
        match self.peek() {
            Tok::Ident(s) => Some(s.as_str()),
            _ => None,
        }
    }

    fn at_kw(&self, k: &str) -> bool {
        self.kw() == Some(k)
    }

    fn eat_kw(&mut self, k: &str) -> bool {
        if self.at_kw(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_kw(&mut self, k: &str) -> PResult<Span> {
        if self.at_kw(k) {
            Ok(self.bump().span)
        } else {
            Err(self.unexpected(&format!("`{}`", k)))
        }
    }

    /// A non-reserved identifier.
    fn ident(&mut self, what: &str) -> PResult<(String, Span)> {
        match self.peek() {
            Tok::Ident(s) if !is_reserved(s) => {
                let name = s.clone();
                let span = self.bump().span;
                Ok((name, span))
            }
            Tok::Ident(s) => Err(Diagnostic::new(
                format!("`{}` is a reserved word and cannot be used as {}", s, what),
                self.span(),
            )),
            _ => Err(self.unexpected(what)),
        }
    }

    /// `a.b.c`
    fn path(&mut self, what: &str) -> PResult<Vec<String>> {
        let mut p = vec![self.ident(what)?.0];
        while self.at(&Tok::Dot) && matches!(self.peek_at(1), Tok::Ident(s) if !is_reserved(s)) {
            self.bump();
            p.push(self.ident(what)?.0);
        }
        Ok(p)
    }

    // -------------------------------------------------------------- items

    fn parse_items(&mut self, in_block: bool) -> PResult<Vec<Item>> {
        let mut items = Vec::new();
        loop {
            if in_block && self.at(&Tok::RBrace) {
                break;
            }
            if self.at(&Tok::Eof) {
                if in_block {
                    return Err(self.unexpected("`}`"));
                }
                break;
            }
            items.push(self.parse_item(in_block)?);
        }
        Ok(items)
    }

    fn parse_item(&mut self, in_block: bool) -> PResult<Item> {
        if self.at_kw("import") {
            let span = self.bump().span;
            let path = self.path("a namespace path")?;
            self.expect(&Tok::Semi)?;
            return Ok(Item::Import(ImportDecl { path, span }));
        }
        if self.at_kw("namespace") {
            return self.parse_namespace(in_block);
        }
        let attributes = self.parse_attributes()?;
        let modifiers = self.parse_modifiers()?;
        match self.kw() {
            Some("class" | "struct" | "record" | "interface") => {
                Ok(Item::Type(self.parse_type_decl(attributes, modifiers)?))
            }
            Some("enum") => Ok(Item::Enum(self.parse_enum(attributes, modifiers)?)),
            Some("func" | "fn" | "function" | "def") => Err(Diagnostic::new(
                "functions cannot be declared at the top level",
                self.span(),
            )
            .with_hint("everything in Iocaste lives inside a class; use a `static` method")),
            _ => Err(self.unexpected(
                "a declaration (`class`, `interface`, `enum`, `struct`, `record`, `namespace` or `import`)",
            )),
        }
    }

    fn parse_namespace(&mut self, in_block: bool) -> PResult<Item> {
        let span = self.bump().span; // namespace
        let name = self.path("a namespace name")?;
        if self.eat(&Tok::Semi) {
            if in_block {
                return Err(Diagnostic::new("a file-scoped `namespace X;` is not allowed inside a namespace block", span));
            }
            let items = self.parse_items(false)?;
            return Ok(Item::Namespace(NamespaceDecl { name, items, file_scoped: true, span }));
        }
        self.expect(&Tok::LBrace)
            .map_err(|d| d.with_hint("write `namespace Name;` for the whole file, or `namespace Name { ... }`"))?;
        let items = self.parse_items(true)?;
        self.expect(&Tok::RBrace)?;
        Ok(Item::Namespace(NamespaceDecl { name, items, file_scoped: false, span }))
    }

    fn parse_attributes(&mut self) -> PResult<Vec<Attribute>> {
        let mut v = Vec::new();
        while self.at(&Tok::At) {
            let span = self.bump().span;
            let name = self.path("an attribute name")?;
            let args = if self.at(&Tok::LParen) { self.parse_args()? } else { Vec::new() };
            v.push(Attribute { name, args, span });
        }
        Ok(v)
    }

    fn parse_modifiers(&mut self) -> PResult<Modifiers> {
        let mut mods: Vec<Modifier> = Vec::new();
        loop {
            let m = match self.peek() {
                Tok::Ident(s) => Modifier::parse(s),
                _ => None,
            };
            let Some(m) = m else { break };
            if mods.contains(&m) {
                return Err(Diagnostic::new(format!("duplicate modifier `{}`", m.as_str()), self.span()));
            }
            self.bump();
            mods.push(m);
        }
        Ok(Modifiers(mods))
    }

    fn parse_generic_params(&mut self) -> PResult<Vec<GenericParam>> {
        if !self.eat(&Tok::Lt) {
            return Ok(Vec::new());
        }
        let mut v = Vec::new();
        loop {
            let (name, span) = self.ident("a type parameter name")?;
            v.push(GenericParam { name, constraints: Vec::new(), span });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::Gt)?;
        Ok(v)
    }

    fn parse_where_clause(&mut self, generics: &mut [GenericParam]) -> PResult<()> {
        self.bump(); // where
        let (name, span) = self.ident("a type parameter name")?;
        self.expect(&Tok::Colon)?;
        let mut constraints = Vec::new();
        loop {
            constraints.push(self.parse_type(false)?);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        match generics.iter_mut().find(|g| g.name == name) {
            Some(g) => {
                g.constraints.extend(constraints);
                Ok(())
            }
            None => Err(Diagnostic::new(format!("`where` refers to unknown type parameter `{}`", name), span)),
        }
    }

    fn parse_type_decl(&mut self, attributes: Vec<Attribute>, modifiers: Modifiers) -> PResult<TypeDecl> {
        let kw_tok = self.bump();
        let span = kw_tok.span;
        let kind = match &kw_tok.tok {
            Tok::Ident(s) if s == "class" => TypeKind::Class,
            Tok::Ident(s) if s == "struct" => TypeKind::Struct,
            Tok::Ident(s) if s == "record" => TypeKind::Record,
            _ => TypeKind::Interface,
        };
        let (name, _) = self.ident("a type name")?;
        let mut generics = self.parse_generic_params()?;
        let record_params = if kind == TypeKind::Record && self.at(&Tok::LParen) {
            self.parse_params()?
        } else {
            Vec::new()
        };
        let mut bases = Vec::new();
        if self.eat(&Tok::Colon) {
            loop {
                bases.push(self.parse_type(false)?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
        }
        while self.at_kw("where") {
            self.parse_where_clause(&mut generics)?;
        }
        let members = if kind == TypeKind::Record && self.eat(&Tok::Semi) {
            Vec::new()
        } else {
            let open = self.expect(&Tok::LBrace)?;
            let m = self.parse_members(open)?;
            self.expect(&Tok::RBrace)?;
            m
        };
        Ok(TypeDecl { attributes, modifiers, kind, name, generics, record_params, bases, members, span })
    }

    fn parse_enum(&mut self, attributes: Vec<Attribute>, modifiers: Modifiers) -> PResult<EnumDecl> {
        let span = self.expect_kw("enum")?;
        let (name, _) = self.ident("an enum name")?;
        let generics = self.parse_generic_params()?;
        self.expect(&Tok::LBrace)?;
        let mut variants = Vec::new();
        while !self.at(&Tok::RBrace) {
            let (vname, vspan) = self.ident("a variant name")?;
            let fields = if self.at(&Tok::LParen) { self.parse_params()? } else { Vec::new() };
            let value = if self.eat(&Tok::Assign) { Some(self.parse_expr()?) } else { None };
            variants.push(EnumVariant { name: vname, fields, value, span: vspan });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RBrace)?;
        Ok(EnumDecl { attributes, modifiers, name, generics, variants, span })
    }

    // ------------------------------------------------------------ members

    fn parse_members(&mut self, open: Span) -> PResult<Vec<Member>> {
        let mut v = Vec::new();
        while !self.at(&Tok::RBrace) {
            if self.at(&Tok::Eof) {
                return Err(Diagnostic::new("unclosed type body: missing `}`", open)
                    .with_hint("this `{` is never closed"));
            }
            v.push(self.parse_member()?);
        }
        Ok(v)
    }

    fn parse_member(&mut self) -> PResult<Member> {
        let attributes = self.parse_attributes()?;
        let modifiers = self.parse_modifiers()?;
        let span = self.span();
        match self.kw() {
            Some("class" | "struct" | "record" | "interface") => {
                return Ok(Member::Type(self.parse_type_decl(attributes, modifiers)?));
            }
            Some("enum") => return Ok(Member::Enum(self.parse_enum(attributes, modifiers)?)),
            Some("constructor") => {
                self.bump();
                let params = self.parse_params()?;
                let body = self.parse_block()?;
                return Ok(Member::Constructor(CtorDecl { attributes, modifiers, params, body, span }));
            }
            Some("destructor") => {
                self.bump();
                self.expect(&Tok::LParen)?;
                self.expect(&Tok::RParen)?;
                let body = self.parse_block()?;
                return Ok(Member::Destructor(DtorDecl { attributes, modifiers, body, span }));
            }
            _ => {}
        }

        let (name, name_span) = self.ident("a member name")?;

        if self.at(&Tok::LParen) || self.at(&Tok::Lt) {
            let generics = self.parse_generic_params()?;
            let params = self.parse_params()?;
            let ret = if self.eat(&Tok::Colon) { Some(self.parse_type(true)?) } else { None };
            let body = self.parse_body()?;
            return Ok(Member::Method(MethodDecl { attributes, modifiers, name, generics, params, ret, body, span: name_span }));
        }

        if self.eat(&Tok::Colon) {
            let ty = self.parse_type(true)?;
            if self.at(&Tok::LBrace) {
                let accessors = self.parse_accessors()?;
                let init = if self.eat(&Tok::Assign) {
                    let e = self.parse_expr()?;
                    self.expect(&Tok::Semi)?;
                    Some(e)
                } else {
                    None
                };
                return Ok(Member::Property(PropertyDecl { attributes, modifiers, name, ty, accessors, init, span: name_span }));
            }
            if self.at(&Tok::Arrow) {
                let aspan = self.bump().span;
                let e = self.parse_expr()?;
                self.expect(&Tok::Semi)?;
                let accessors = vec![Accessor {
                    kind: AccessorKind::Get,
                    modifiers: Modifiers::default(),
                    body: Body::Expr(e),
                    span: aspan,
                }];
                return Ok(Member::Property(PropertyDecl { attributes, modifiers, name, ty, accessors, init: None, span: name_span }));
            }
            let init = if self.eat(&Tok::Assign) { Some(self.parse_expr()?) } else { None };
            self.expect(&Tok::Semi)?;
            return Ok(Member::Field(FieldDecl { attributes, modifiers, name, ty, init, span: name_span }));
        }

        let mut d = self.unexpected("`(` or `:` after the member name");
        if matches!(self.peek(), Tok::Ident(s) if !is_reserved(s)) {
            d = d.with_hint("in Iocaste the type comes after the name, e.g. `count: int` or `run(): void`");
        }
        Err(d)
    }

    fn parse_accessors(&mut self) -> PResult<Vec<Accessor>> {
        let open = self.expect(&Tok::LBrace)?;
        let mut v = Vec::new();
        while !self.at(&Tok::RBrace) {
            let modifiers = self.parse_modifiers()?;
            let span = self.span();
            let kind = match self.kw() {
                Some("get") => AccessorKind::Get,
                Some("set") => AccessorKind::Set,
                _ => return Err(self.unexpected("`get` or `set`")),
            };
            self.bump();
            let body = self.parse_body()?;
            v.push(Accessor { kind, modifiers, body, span });
        }
        self.expect(&Tok::RBrace)?;
        if v.is_empty() {
            return Err(Diagnostic::new("a property needs at least one accessor", open)
                .with_hint("write `{ get; }` or `{ get; set; }`"));
        }
        Ok(v)
    }

    /// Method / accessor body: `=> expr;`, `{ ... }` or `;`.
    fn parse_body(&mut self) -> PResult<Body> {
        if self.eat(&Tok::Arrow) {
            let e = self.parse_expr()?;
            self.expect(&Tok::Semi)?;
            Ok(Body::Expr(e))
        } else if self.at(&Tok::LBrace) {
            Ok(Body::Block(self.parse_block()?))
        } else if self.eat(&Tok::Semi) {
            Ok(Body::None)
        } else {
            Err(self.unexpected("`=>`, `{` or `;`"))
        }
    }

    fn parse_params(&mut self) -> PResult<Vec<Param>> {
        self.expect(&Tok::LParen)?;
        let mut v = Vec::new();
        while !self.at(&Tok::RParen) {
            let (name, span) = self.ident("a parameter name")?;
            self.expect(&Tok::Colon)
                .map_err(|d| d.with_hint("parameters are written `name: Type`"))?;
            let ty = self.parse_type(true)?;
            let default = if self.eat(&Tok::Assign) { Some(self.parse_expr()?) } else { None };
            v.push(Param { name, ty, default, span });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen)?;
        Ok(v)
    }

    // -------------------------------------------------------------- types

    fn parse_type(&mut self, allow_suffix: bool) -> PResult<TypeRef> {
        let span = self.span();
        if self.eat_kw("ref") {
            let inner = self.parse_type(allow_suffix)?;
            return Ok(TypeRef { kind: TypeRefKind::Ref { mutable: false, inner: Box::new(inner) }, span });
        }
        if self.at_kw("mut") {
            self.bump();
            self.expect_kw("ref")?;
            let inner = self.parse_type(allow_suffix)?;
            return Ok(TypeRef { kind: TypeRefKind::Ref { mutable: true, inner: Box::new(inner) }, span });
        }
        let path = self.path("a type name")?;
        let args = if self.at(&Tok::Lt) { self.parse_type_args()? } else { Vec::new() };
        let mut ty = TypeRef { kind: TypeRefKind::Named { path, args }, span };
        if allow_suffix {
            loop {
                if self.at(&Tok::LBracket) && matches!(self.peek_at(1), Tok::RBracket) {
                    self.bump();
                    self.bump();
                    ty = TypeRef { kind: TypeRefKind::Array(Box::new(ty)), span };
                } else if self.at(&Tok::Question) {
                    self.bump();
                    ty = TypeRef { kind: TypeRefKind::Optional(Box::new(ty)), span };
                } else {
                    break;
                }
            }
        }
        Ok(ty)
    }

    fn parse_type_args(&mut self) -> PResult<Vec<TypeRef>> {
        self.expect(&Tok::Lt)?;
        let mut v = Vec::new();
        loop {
            v.push(self.parse_type(true)?);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::Gt)?;
        Ok(v)
    }

    // --------------------------------------------------------- statements

    fn parse_block(&mut self) -> PResult<Block> {
        let span = self.expect(&Tok::LBrace)?;
        let mut stmts = Vec::new();
        while !self.at(&Tok::RBrace) {
            if self.at(&Tok::Eof) {
                return Err(Diagnostic::new("unclosed block: missing `}`", span)
                    .with_hint("this `{` is never closed"));
            }
            stmts.push(self.parse_stmt()?);
        }
        self.expect(&Tok::RBrace)?;
        Ok(Block { stmts, span })
    }

    fn parse_local_rest(&mut self, mutable: bool, name: String, span: Span) -> PResult<Stmt> {
        let ty = if self.eat(&Tok::Colon) { Some(self.parse_type(true)?) } else { None };
        let init = if self.eat(&Tok::Assign) { Some(self.parse_expr()?) } else { None };
        if ty.is_none() && init.is_none() {
            return Err(Diagnostic::new(format!("cannot infer the type of `{}`", name), span)
                .with_hint("add a type (`: Type`) or an initial value (`= ...`)"));
        }
        Ok(Stmt { kind: StmtKind::Local { mutable, name, ty, init }, span })
    }

    fn parse_paren_cond(&mut self) -> PResult<Expr> {
        self.expect(&Tok::LParen)?;
        let e = self.parse_expr()?;
        self.expect(&Tok::RParen)?;
        Ok(e)
    }

    fn parse_stmt(&mut self) -> PResult<Stmt> {
        let span = self.span();
        if self.at(&Tok::LBrace) {
            let b = self.parse_block()?;
            return Ok(Stmt { kind: StmtKind::Block(b), span });
        }
        match self.kw() {
            Some("let" | "var") => {
                let mutable = self.at_kw("var");
                self.bump();
                let (name, _) = self.ident("a variable name")?;
                let s = self.parse_local_rest(mutable, name, span)?;
                self.expect(&Tok::Semi)?;
                return Ok(s);
            }
            Some("if") => {
                self.bump();
                let cond = self.parse_paren_cond()?;
                let then = Box::new(self.parse_stmt()?);
                let otherwise = if self.eat_kw("else") { Some(Box::new(self.parse_stmt()?)) } else { None };
                return Ok(Stmt { kind: StmtKind::If { cond, then, otherwise }, span });
            }
            Some("while") => {
                self.bump();
                let cond = self.parse_paren_cond()?;
                let body = Box::new(self.parse_stmt()?);
                return Ok(Stmt { kind: StmtKind::While { cond, body }, span });
            }
            Some("for") => return self.parse_for(span),
            Some("return") => {
                self.bump();
                let value = if self.at(&Tok::Semi) { None } else { Some(self.parse_expr()?) };
                self.expect(&Tok::Semi)?;
                return Ok(Stmt { kind: StmtKind::Return(value), span });
            }
            Some("break") => {
                self.bump();
                self.expect(&Tok::Semi)?;
                return Ok(Stmt { kind: StmtKind::Break, span });
            }
            Some("continue") => {
                self.bump();
                self.expect(&Tok::Semi)?;
                return Ok(Stmt { kind: StmtKind::Continue, span });
            }
            Some("unsafe") => {
                self.bump();
                let b = self.parse_block()?;
                return Ok(Stmt { kind: StmtKind::Unsafe(b), span });
            }
            Some("match" | "spawn") => {
                // block-like expression statements: the trailing `;` is optional
                let e = self.parse_primary()?;
                self.eat(&Tok::Semi);
                return Ok(Stmt { kind: StmtKind::Expr(e), span });
            }
            Some("try" | "throw" | "catch" | "finally") => {
                return Err(Diagnostic::new("Iocaste has no exceptions", span)
                    .with_hint("return `Result<T, E>` and propagate errors with the `?` operator"));
            }
            _ => {}
        }
        let e = self.parse_expr()?;
        self.expect(&Tok::Semi)?;
        Ok(Stmt { kind: StmtKind::Expr(e), span })
    }

    fn parse_for(&mut self, span: Span) -> PResult<Stmt> {
        self.bump(); // for
        self.expect(&Tok::LParen)?;
        let init = if self.at(&Tok::Semi) {
            None
        } else if self.at_kw("let") || self.at_kw("var") {
            let mutable = self.at_kw("var");
            let ispan = self.span();
            self.bump();
            let (name, _) = self.ident("a variable name")?;
            if self.eat_kw("in") {
                let iter = self.parse_expr()?;
                self.expect(&Tok::RParen)?;
                let body = Box::new(self.parse_stmt()?);
                return Ok(Stmt { kind: StmtKind::ForIn { mutable, name, iter, body }, span });
            }
            Some(Box::new(self.parse_local_rest(mutable, name, ispan)?))
        } else {
            let espan = self.span();
            let e = self.parse_expr()?;
            Some(Box::new(Stmt { kind: StmtKind::Expr(e), span: espan }))
        };
        self.expect(&Tok::Semi)?;
        let cond = if self.at(&Tok::Semi) { None } else { Some(self.parse_expr()?) };
        self.expect(&Tok::Semi)?;
        let update = if self.at(&Tok::RParen) { None } else { Some(self.parse_expr()?) };
        self.expect(&Tok::RParen)?;
        let body = Box::new(self.parse_stmt()?);
        Ok(Stmt { kind: StmtKind::For { init, cond, update, body }, span })
    }

    // -------------------------------------------------------- expressions

    fn parse_expr(&mut self) -> PResult<Expr> {
        self.parse_assign()
    }

    fn parse_assign(&mut self) -> PResult<Expr> {
        let lhs = self.parse_coalesce()?;
        let op = match self.peek() {
            Tok::Assign => Some(AssignOp::Assign),
            Tok::PlusAssign => Some(AssignOp::Add),
            Tok::MinusAssign => Some(AssignOp::Sub),
            Tok::StarAssign => Some(AssignOp::Mul),
            Tok::SlashAssign => Some(AssignOp::Div),
            Tok::PercentAssign => Some(AssignOp::Rem),
            _ => None,
        };
        let Some(op) = op else { return Ok(lhs) };
        if !matches!(lhs.kind, ExprKind::Ident(_) | ExprKind::Member { .. } | ExprKind::Index { .. }) {
            return Err(Diagnostic::new("invalid assignment target", lhs.span)
                .with_hint("you can assign to a variable, a field/property or an indexed element"));
        }
        let span = lhs.span;
        self.bump();
        let value = self.parse_assign()?;
        Ok(Expr { kind: ExprKind::Assign { op, target: Box::new(lhs), value: Box::new(value) }, span })
    }

    fn parse_coalesce(&mut self) -> PResult<Expr> {
        let lhs = self.parse_binary(0)?;
        if self.at(&Tok::QuestionQuestion) {
            let span = lhs.span;
            self.bump();
            let rhs = self.parse_coalesce()?;
            return Ok(Expr {
                kind: ExprKind::Binary { op: BinOp::Coalesce, lhs: Box::new(lhs), rhs: Box::new(rhs) },
                span,
            });
        }
        Ok(lhs)
    }

    fn binop_at(&self, level: usize) -> Option<BinOp> {
        match (level, self.peek()) {
            (0, Tok::OrOr) => Some(BinOp::Or),
            (1, Tok::AndAnd) => Some(BinOp::And),
            (2, Tok::EqEq) => Some(BinOp::Eq),
            (2, Tok::NotEq) => Some(BinOp::Ne),
            (3, Tok::Lt) => Some(BinOp::Lt),
            (3, Tok::Gt) => Some(BinOp::Gt),
            (3, Tok::LtEq) => Some(BinOp::Le),
            (3, Tok::GtEq) => Some(BinOp::Ge),
            (4, Tok::Plus) => Some(BinOp::Add),
            (4, Tok::Minus) => Some(BinOp::Sub),
            (5, Tok::Star) => Some(BinOp::Mul),
            (5, Tok::Slash) => Some(BinOp::Div),
            (5, Tok::Percent) => Some(BinOp::Rem),
            _ => None,
        }
    }

    /// Binary operators, lowest precedence (`||`, level 0) to highest (`* / %`, level 5).
    fn parse_binary(&mut self, level: usize) -> PResult<Expr> {
        if level > 5 {
            return self.parse_unary();
        }
        let mut lhs = self.parse_binary(level + 1)?;
        while let Some(op) = self.binop_at(level) {
            let span = lhs.span;
            self.bump();
            let rhs = self.parse_binary(level + 1)?;
            lhs = Expr { kind: ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> PResult<Expr> {
        let span = self.span();
        if self.eat(&Tok::Minus) {
            let e = self.parse_unary()?;
            return Ok(Expr { kind: ExprKind::Unary { op: UnOp::Neg, expr: Box::new(e) }, span });
        }
        if self.eat(&Tok::Bang) {
            let e = self.parse_unary()?;
            return Ok(Expr { kind: ExprKind::Unary { op: UnOp::Not, expr: Box::new(e) }, span });
        }
        if self.eat_kw("ref") {
            let e = self.parse_unary()?;
            return Ok(Expr { kind: ExprKind::Borrow { mutable: false, expr: Box::new(e) }, span });
        }
        if self.at_kw("mut") {
            self.bump();
            self.expect_kw("ref")?;
            let e = self.parse_unary()?;
            return Ok(Expr { kind: ExprKind::Borrow { mutable: true, expr: Box::new(e) }, span });
        }
        if self.eat_kw("await") {
            let e = self.parse_unary()?;
            return Ok(Expr { kind: ExprKind::Await(Box::new(e)), span });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut e = self.parse_primary()?;
        loop {
            let span = e.span;
            match self.peek() {
                Tok::Dot => {
                    self.bump();
                    let (name, _) = self.ident("a member name")?;
                    e = Expr { kind: ExprKind::Member { target: Box::new(e), name }, span };
                }
                Tok::LParen => {
                    let args = self.parse_args()?;
                    e = Expr { kind: ExprKind::Call { callee: Box::new(e), args }, span };
                }
                Tok::LBracket => {
                    self.bump();
                    let index = self.parse_expr()?;
                    self.expect(&Tok::RBracket)?;
                    e = Expr { kind: ExprKind::Index { target: Box::new(e), index: Box::new(index) }, span };
                }
                Tok::Question => {
                    self.bump();
                    e = Expr { kind: ExprKind::Try(Box::new(e)), span };
                }
                Tok::PlusPlus => {
                    self.bump();
                    e = Expr { kind: ExprKind::Postfix { op: PostOp::Inc, expr: Box::new(e) }, span };
                }
                Tok::MinusMinus => {
                    self.bump();
                    e = Expr { kind: ExprKind::Postfix { op: PostOp::Dec, expr: Box::new(e) }, span };
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn parse_args(&mut self) -> PResult<Vec<Expr>> {
        self.expect(&Tok::LParen)?;
        let mut v = Vec::new();
        while !self.at(&Tok::RParen) {
            v.push(self.parse_expr()?);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen)?;
        Ok(v)
    }

    /// Parse the source of a `{...}` hole of an interpolated string.
    fn parse_embedded(src: &str, at: Span) -> PResult<Expr> {
        let toks = lexer::lex_at(src, at.line, at.col)?;
        let mut p = Parser::new(toks);
        let e = p.parse_expr()?;
        if !p.at(&Tok::Eof) {
            return Err(p.unexpected("`}` after the interpolated expression"));
        }
        Ok(e)
    }

    fn parse_primary(&mut self) -> PResult<Expr> {
        let span = self.span();
        let tok = self.peek().clone();
        let kind = match tok {
            Tok::Int { text, suffix } => {
                self.bump();
                ExprKind::Int { text, suffix }
            }
            Tok::Float { text, suffix } => {
                self.bump();
                ExprKind::Float { text, suffix }
            }
            Tok::Str(s) => {
                self.bump();
                ExprKind::Str(s)
            }
            Tok::Char(c) => {
                self.bump();
                ExprKind::Char(c)
            }
            Tok::Interp(parts) => {
                self.bump();
                let mut segs = Vec::new();
                for p in parts {
                    match p {
                        InterpPart::Text(t) => segs.push(InterpSegment::Text(t)),
                        InterpPart::Expr { src, span: es } => {
                            segs.push(InterpSegment::Expr(Box::new(Self::parse_embedded(&src, es)?)));
                        }
                    }
                }
                ExprKind::Interp(segs)
            }
            Tok::LParen => {
                self.bump();
                let e = self.parse_expr()?;
                self.expect(&Tok::RParen)?;
                return Ok(e);
            }
            Tok::LBracket => {
                self.bump();
                let mut items = Vec::new();
                while !self.at(&Tok::RBracket) {
                    items.push(self.parse_expr()?);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(&Tok::RBracket)?;
                ExprKind::Array(items)
            }
            Tok::Ident(s) => match s.as_str() {
                "true" => {
                    self.bump();
                    ExprKind::Bool(true)
                }
                "false" => {
                    self.bump();
                    ExprKind::Bool(false)
                }
                "this" => {
                    self.bump();
                    ExprKind::This
                }
                "base" => {
                    self.bump();
                    ExprKind::Base
                }
                "new" => {
                    self.bump();
                    let ty = self.parse_type(false)?;
                    if !self.at(&Tok::LParen) {
                        return Err(self.unexpected("`(` after the type in `new`"));
                    }
                    let args = self.parse_args()?;
                    ExprKind::New { ty, args }
                }
                "match" => self.parse_match()?,
                "spawn" => {
                    self.bump();
                    ExprKind::Spawn(self.parse_block()?)
                }
                "return" => {
                    self.bump();
                    let ends = matches!(
                        self.peek(),
                        Tok::Semi | Tok::RParen | Tok::RBrace | Tok::RBracket | Tok::Comma | Tok::Eof
                    );
                    let value = if ends { None } else { Some(Box::new(self.parse_expr()?)) };
                    ExprKind::Return(value)
                }
                "null" => {
                    return Err(Diagnostic::new("Iocaste has no `null`", span)
                        .with_hint("use `Option<T>` (written `T?`) for values that may be absent"));
                }
                "try" | "throw" | "catch" | "finally" => {
                    return Err(Diagnostic::new("Iocaste has no exceptions", span)
                        .with_hint("return `Result<T, E>` and propagate errors with the `?` operator"));
                }
                _ if is_reserved(&s) => {
                    return Err(Diagnostic::new(format!("expected an expression, found keyword `{}`", s), span));
                }
                _ => {
                    self.bump();
                    ExprKind::Ident(s)
                }
            },
            other => {
                return Err(Diagnostic::new(format!("expected an expression, found {}", other.describe()), span));
            }
        };
        Ok(Expr { kind, span })
    }

    fn parse_match(&mut self) -> PResult<ExprKind> {
        self.bump(); // match
        let scrutinee = self.parse_paren_cond()?;
        let open = self.expect(&Tok::LBrace)?;
        let mut arms = Vec::new();
        while !self.at(&Tok::RBrace) {
            if self.at(&Tok::Eof) {
                return Err(Diagnostic::new("unclosed `match`: missing `}`", open));
            }
            let aspan = self.span();
            let pattern = self.parse_pattern()?;
            let guard = if self.eat_kw("if") { Some(self.parse_expr()?) } else { None };
            self.expect(&Tok::Arrow)?;
            let (body, was_block) = if self.at(&Tok::LBrace) {
                let bspan = self.span();
                let b = self.parse_block()?;
                (Expr { kind: ExprKind::Block(b), span: bspan }, true)
            } else {
                (self.parse_expr()?, false)
            };
            arms.push(MatchArm { pattern, guard, body, span: aspan });
            let comma = self.eat(&Tok::Comma);
            if !comma && !was_block && !self.at(&Tok::RBrace) {
                return Err(self.unexpected("`,` or `}` after the match arm"));
            }
        }
        self.expect(&Tok::RBrace)?;
        Ok(ExprKind::Match { scrutinee: Box::new(scrutinee), arms })
    }

    fn parse_pattern(&mut self) -> PResult<Pattern> {
        let span = self.span();
        let tok = self.peek().clone();
        let kind = match tok {
            Tok::Int { .. } | Tok::Float { .. } | Tok::Str(_) | Tok::Char(_) => {
                PatternKind::Literal(self.parse_primary()?)
            }
            Tok::Minus => {
                self.bump();
                if !matches!(self.peek(), Tok::Int { .. } | Tok::Float { .. }) {
                    return Err(self.unexpected("a number after `-` in a pattern"));
                }
                let e = self.parse_primary()?;
                PatternKind::Literal(Expr { kind: ExprKind::Unary { op: UnOp::Neg, expr: Box::new(e) }, span })
            }
            Tok::Ident(s) if s == "true" || s == "false" => PatternKind::Literal(self.parse_primary()?),
            Tok::Ident(s) if s == "_" => {
                self.bump();
                PatternKind::Wildcard
            }
            Tok::Ident(_) => {
                let path = self.path("a pattern")?;
                if self.eat(&Tok::LParen) {
                    let mut args = Vec::new();
                    while !self.at(&Tok::RParen) {
                        args.push(self.parse_pattern()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(&Tok::RParen)?;
                    PatternKind::Variant { path, args }
                } else if path.len() == 1 {
                    PatternKind::Binding(path.into_iter().next().unwrap())
                } else {
                    PatternKind::Path(path)
                }
            }
            other => {
                return Err(Diagnostic::new(format!("expected a pattern, found {}", other.describe()), span));
            }
        };
        Ok(Pattern { kind, span })
    }
}
