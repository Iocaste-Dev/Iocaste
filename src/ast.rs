//! Abstract syntax tree for Iocaste v0.0.2-alpha.

use crate::diag::Span;

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub enum Item {
    Namespace(NamespaceDecl),
    Import(ImportDecl),
    Type(TypeDecl),
    Enum(EnumDecl),
}

#[derive(Debug, Clone)]
pub struct NamespaceDecl {
    pub name: Vec<String>,
    pub items: Vec<Item>,
    /// `namespace A.B;` (covers the rest of the file) vs `namespace A.B { ... }`
    pub file_scoped: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ImportDecl {
    pub path: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Attribute {
    pub name: Vec<String>,
    pub args: Vec<Expr>,
    pub span: Span,
}

// ---------------------------------------------------------------- modifiers

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Public,
    Private,
    Protected,
    Internal,
    Static,
    Abstract,
    Override,
    Virtual,
    Sealed,
    Readonly,
    Async,
    Partial,
    Unsafe,
}

impl Modifier {
    pub fn parse(s: &str) -> Option<Modifier> {
        Some(match s {
            "public" => Modifier::Public,
            "private" => Modifier::Private,
            "protected" => Modifier::Protected,
            "internal" => Modifier::Internal,
            "static" => Modifier::Static,
            "abstract" => Modifier::Abstract,
            "override" => Modifier::Override,
            "virtual" => Modifier::Virtual,
            "sealed" => Modifier::Sealed,
            "readonly" => Modifier::Readonly,
            "async" => Modifier::Async,
            "partial" => Modifier::Partial,
            "unsafe" => Modifier::Unsafe,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Modifier::Public => "public",
            Modifier::Private => "private",
            Modifier::Protected => "protected",
            Modifier::Internal => "internal",
            Modifier::Static => "static",
            Modifier::Abstract => "abstract",
            Modifier::Override => "override",
            Modifier::Virtual => "virtual",
            Modifier::Sealed => "sealed",
            Modifier::Readonly => "readonly",
            Modifier::Async => "async",
            Modifier::Partial => "partial",
            Modifier::Unsafe => "unsafe",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Modifiers(pub Vec<Modifier>);

impl Modifiers {
    pub fn contains(&self, m: Modifier) -> bool {
        self.0.contains(&m)
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

// ------------------------------------------------------------------ types

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    Class,
    Struct,
    Record,
    Interface,
}

#[derive(Debug, Clone)]
pub struct GenericParam {
    pub name: String,
    pub constraints: Vec<TypeRef>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TypeDecl {
    pub attributes: Vec<Attribute>,
    pub modifiers: Modifiers,
    pub kind: TypeKind,
    pub name: String,
    pub generics: Vec<GenericParam>,
    /// Primary-constructor parameters of a `record`.
    pub record_params: Vec<Param>,
    /// Base class and/or interfaces (`: Base, IFoo`).
    pub bases: Vec<TypeRef>,
    pub members: Vec<Member>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub attributes: Vec<Attribute>,
    pub modifiers: Modifiers,
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub variants: Vec<EnumVariant>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: String,
    /// Data carried by the variant, e.g. `NotANumber(text: string)`.
    pub fields: Vec<Param>,
    pub value: Option<Expr>,
    pub span: Span,
}

/// A reference to a type in source code (`int`, `List<T>`, `string[]`, `T?`, `mut ref Player`).
#[derive(Debug, Clone)]
pub struct TypeRef {
    pub kind: TypeRefKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TypeRefKind {
    Named { path: Vec<String>, args: Vec<TypeRef> },
    Array(Box<TypeRef>),
    Optional(Box<TypeRef>),
    Ref { mutable: bool, inner: Box<TypeRef> },
}

// ---------------------------------------------------------------- members

#[derive(Debug, Clone)]
pub enum Member {
    Field(FieldDecl),
    Property(PropertyDecl),
    Method(MethodDecl),
    Constructor(CtorDecl),
    Destructor(DtorDecl),
    Type(TypeDecl),
    Enum(EnumDecl),
}

#[derive(Debug, Clone)]
pub struct FieldDecl {
    pub attributes: Vec<Attribute>,
    pub modifiers: Modifiers,
    pub name: String,
    pub ty: TypeRef,
    pub init: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessorKind {
    Get,
    Set,
}

#[derive(Debug, Clone)]
pub struct Accessor {
    pub kind: AccessorKind,
    pub modifiers: Modifiers,
    pub body: Body,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct PropertyDecl {
    pub attributes: Vec<Attribute>,
    pub modifiers: Modifiers,
    pub name: String,
    pub ty: TypeRef,
    pub accessors: Vec<Accessor>,
    pub init: Option<Expr>,
    pub span: Span,
}

/// Body of a method or accessor.
#[derive(Debug, Clone)]
pub enum Body {
    /// `;` (abstract / interface member / auto accessor)
    None,
    /// `=> expr;`
    Expr(Expr),
    /// `{ ... }`
    Block(Block),
}

#[derive(Debug, Clone)]
pub struct MethodDecl {
    pub attributes: Vec<Attribute>,
    pub modifiers: Modifiers,
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub ret: Option<TypeRef>,
    pub body: Body,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct CtorDecl {
    pub attributes: Vec<Attribute>,
    pub modifiers: Modifiers,
    pub params: Vec<Param>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct DtorDecl {
    pub attributes: Vec<Attribute>,
    pub modifiers: Modifiers,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: TypeRef,
    pub default: Option<Expr>,
    pub span: Span,
}

// ------------------------------------------------------------- statements

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    /// `let x = ...;` (immutable) or `var x = ...;` (mutable)
    Local { mutable: bool, name: String, ty: Option<TypeRef>, init: Option<Expr> },
    Expr(Expr),
    Block(Block),
    If { cond: Expr, then: Box<Stmt>, otherwise: Option<Box<Stmt>> },
    While { cond: Expr, body: Box<Stmt> },
    For { init: Option<Box<Stmt>>, cond: Option<Expr>, update: Option<Expr>, body: Box<Stmt> },
    ForIn { mutable: bool, name: String, iter: Expr, body: Box<Stmt> },
    Return(Option<Expr>),
    Break,
    Continue,
    Unsafe(Block),
}

// ------------------------------------------------------------ expressions

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum InterpSegment {
    Text(String),
    Expr(Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    /// `a ?? b`
    Coalesce,
}

impl BinOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Gt => ">",
            BinOp::Le => "<=",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
            BinOp::Coalesce => "??",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl AssignOp {
    pub fn as_str(self) -> &'static str {
        match self {
            AssignOp::Assign => "=",
            AssignOp::Add => "+=",
            AssignOp::Sub => "-=",
            AssignOp::Mul => "*=",
            AssignOp::Div => "/=",
            AssignOp::Rem => "%=",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostOp {
    Inc,
    Dec,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int { text: String, suffix: String },
    Float { text: String, suffix: String },
    Str(String),
    Interp(Vec<InterpSegment>),
    Char(char),
    Bool(bool),
    Ident(String),
    This,
    Base,
    Member { target: Box<Expr>, name: String },
    Index { target: Box<Expr>, index: Box<Expr> },
    Call { callee: Box<Expr>, args: Vec<Expr> },
    New { ty: TypeRef, args: Vec<Expr> },
    Unary { op: UnOp, expr: Box<Expr> },
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr> },
    Assign { op: AssignOp, target: Box<Expr>, value: Box<Expr> },
    Postfix { op: PostOp, expr: Box<Expr> },
    /// `expr?` : propagate the error of a `Result` / `Option`
    Try(Box<Expr>),
    Await(Box<Expr>),
    /// `ref x` / `mut ref x`
    Borrow { mutable: bool, expr: Box<Expr> },
    Match { scrutinee: Box<Expr>, arms: Vec<MatchArm> },
    /// `spawn { ... }` : start a lightweight task
    Spawn(Block),
    Block(Block),
    Return(Option<Box<Expr>>),
    Array(Vec<Expr>),
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum PatternKind {
    /// `_`
    Wildcard,
    /// `0`, `"x"`, `'c'`, `true`, `-1`
    Literal(Expr),
    /// A single lowercase-or-uppercase name: binds a new variable
    /// (a later pass decides whether it names a unit variant instead).
    Binding(String),
    /// `Status.Active`
    Path(Vec<String>),
    /// `Ok(value)`, `Status.Banned(reason)`
    Variant { path: Vec<String>, args: Vec<Pattern> },
}
