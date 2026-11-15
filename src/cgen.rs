//! C backend (v0.0.5-alpha): turns a type-checked program into one C file.
//!
//! Supported so far: classes made of `static` methods; the primitive types, `string`, `string[]`
//! (command-line arguments); `let`/`var`; `if`/`while`/`for`; arithmetic; string interpolation and
//! `+`; calls between static methods (with overloads); `Console` and `Math`; a few `string` methods.
//! Everything else reports "not supported by the C backend yet".

use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use std::collections::HashMap;

pub const RUNTIME: &str = include_str!("runtime.h");

pub struct CError {
    pub file: usize,
    pub diag: Diagnostic,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CT {
    Bool,
    Char,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Str,
    StrArr,
    Void,
}

impl CT {
    fn c(self) -> &'static str {
        match self {
            CT::Bool => "bool",
            CT::Char => "uint32_t",
            CT::I8 => "int8_t",
            CT::I16 => "int16_t",
            CT::I32 => "int32_t",
            CT::I64 => "int64_t",
            CT::U8 => "uint8_t",
            CT::U16 => "uint16_t",
            CT::U32 => "uint32_t",
            CT::U64 => "uint64_t",
            CT::F32 => "float",
            CT::F64 => "double",
            CT::Str => "ioc_string",
            CT::StrArr => "ioc_strarr",
            CT::Void => "void",
        }
    }
    fn is_int(self) -> bool {
        matches!(self, CT::I8 | CT::I16 | CT::I32 | CT::I64 | CT::U8 | CT::U16 | CT::U32 | CT::U64)
    }
    fn is_float(self) -> bool {
        matches!(self, CT::F32 | CT::F64)
    }
    fn is_num(self) -> bool {
        self.is_int() || self.is_float()
    }
    fn is_unsigned(self) -> bool {
        matches!(self, CT::U8 | CT::U16 | CT::U32 | CT::U64)
    }
}

fn promote(a: CT, b: CT) -> CT {
    if a == CT::F64 || b == CT::F64 {
        CT::F64
    } else if a.is_float() || b.is_float() {
        CT::F32
    } else if a == CT::U64 || b == CT::U64 {
        CT::U64
    } else if a == CT::I64 || b == CT::I64 {
        CT::I64
    } else if a == CT::U32 || b == CT::U32 {
        CT::U32
    } else {
        CT::I32
    }
}

struct Method<'a> {
    class: String,
    name: String,
    c_name: String,
    params: Vec<(String, CT)>,
    ret: CT,
    decl: &'a MethodDecl,
    file: usize,
}

struct Gen<'a> {
    methods: Vec<Method<'a>>,
    errors: Vec<CError>,
    file: usize,
    class: String,
    scopes: Vec<HashMap<String, CT>>,
    ret: CT,
    out: String,
    ind: usize,
}

fn type_ct(t: &TypeRef) -> Option<CT> {
    match &t.kind {
        TypeRefKind::Named { path, args } if path.len() == 1 && args.is_empty() => Some(match path[0].as_str() {
            "bool" => CT::Bool,
            "char" => CT::Char,
            "sbyte" => CT::I8,
            "short" => CT::I16,
            "int" => CT::I32,
            "long" | "isize" => CT::I64,
            "byte" => CT::U8,
            "ushort" => CT::U16,
            "uint" => CT::U32,
            "ulong" | "usize" => CT::U64,
            "float" => CT::F32,
            "double" => CT::F64,
            "string" => CT::Str,
            "void" => CT::Void,
            _ => return None,
        }),
        TypeRefKind::Array(inner) => match type_ct(inner) {
            Some(CT::Str) => Some(CT::StrArr),
            _ => None,
        },
        _ => None,
    }
}

fn c_string(s: &str) -> String {
    let mut o = String::from("\"");
    for b in s.bytes() {
        match b {
            b'\\' => o.push_str("\\\\"),
            b'"' => o.push_str("\\\""),
            b'\n' => o.push_str("\\n"),
            b'\r' => o.push_str("\\r"),
            b'\t' => o.push_str("\\t"),
            0x20..=0x7e if b != b'?' => o.push(b as char),
            _ => o.push_str(&format!("\\{:03o}", b)),
        }
    }
    o.push('"');
    o
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

/// Generate C for all `files`. Assumes the program already passed the type and ownership checks.
pub fn generate(files: &[&SourceFile]) -> Result<String, Vec<CError>> {
    let mut g = Gen {
        methods: Vec::new(),
        errors: Vec::new(),
        file: 0,
        class: String::new(),
        scopes: Vec::new(),
        ret: CT::Void,
        out: String::new(),
        ind: 0,
    };
    for (i, f) in files.iter().enumerate() {
        g.file = i;
        g.collect_items(&f.items);
    }
    if !g.errors.is_empty() {
        return Err(g.errors);
    }
    let entries: Vec<usize> =
        g.methods.iter().enumerate().filter(|(_, m)| m.name == "main").map(|(i, _)| i).collect();
    let Some(&entry) = entries.first() else {
        return Err(vec![CError {
            file: 0,
            diag: Diagnostic::new("no entry point: there is no `static main` method", Span::new(1, 1))
                .with_hint("add `public static main(args: string[]): int { ... }` to a class"),
        }]);
    };
    if entries.len() > 1 {
        let m = &g.methods[entries[1]];
        return Err(vec![CError {
            file: m.file,
            diag: Diagnostic::new("there is more than one `main` method", m.decl.span)
                .with_hint("a program has exactly one entry point"),
        }]);
    }
    let e = &g.methods[entry];
    let bad_entry = !(matches!(e.ret, CT::I32 | CT::Void)
        && (e.params.is_empty() || (e.params.len() == 1 && e.params[0].1 == CT::StrArr)));
    if bad_entry {
        return Err(vec![CError {
            file: e.file,
            diag: Diagnostic::new("`main` must be `main(): int`, `main(args: string[]): int` (or return `void`)", e.decl.span),
        }]);
    }

    g.out.push_str(RUNTIME);
    g.out.push_str("\n/* ---- program */\n");
    for i in 0..g.methods.len() {
        let sig = g.signature(i);
        g.out.push_str(&format!("static {};\n", sig));
    }
    g.out.push('\n');
    for i in 0..g.methods.len() {
        g.emit_method(i);
    }
    if !g.errors.is_empty() {
        return Err(g.errors);
    }
    let e = &g.methods[entry];
    let call_args = if e.params.is_empty() { "" } else { "args" };
    let call = format!("{}({})", e.c_name, call_args);
    let body = if e.ret == CT::Void { format!("{};\n    return 0;", call) } else { format!("return (int){};", call) };
    g.out.push_str(&format!(
        "\nint main(int argc, char **argv) {{\n    ioc_strarr args = {{ argc > 0 ? argc - 1 : 0, (ioc_string *)(argv + 1) }};\n    (void)args;\n    {}\n}}\n",
        body
    ));
    Ok(g.out)
}

impl<'a> Gen<'a> {
    fn err(&mut self, span: Span, msg: impl Into<String>) {
        self.errors.push(CError { file: self.file, diag: Diagnostic::new(msg, span) });
    }

    fn unsupported(&mut self, span: Span, what: &str) {
        self.errors.push(CError {
            file: self.file,
            diag: Diagnostic::new(format!("{} is not supported by the C backend yet (v0.0.5)", what), span)
                .with_hint("`ioc check` still validates it; code generation for it arrives in the next versions"),
        });
    }

    // ------------------------------------------------------- collecting

    fn collect_items(&mut self, items: &'a [Item]) {
        for it in items {
            match it {
                Item::Namespace(n) => self.collect_items(&n.items),
                Item::Import(_) => {}
                Item::Enum(e) => self.unsupported(e.span, "`enum`"),
                Item::Type(t) => self.collect_type(t),
            }
        }
    }

    fn collect_type(&mut self, t: &'a TypeDecl) {
        if t.kind != TypeKind::Class {
            self.unsupported(t.span, &format!("`{}`", match t.kind {
                TypeKind::Struct => "struct",
                TypeKind::Record => "record",
                _ => "interface",
            }));
            return;
        }
        if !t.generics.is_empty() {
            self.unsupported(t.span, "a generic class");
        }
        if !t.bases.is_empty() {
            self.unsupported(t.span, "inheritance");
        }
        for m in &t.members {
            match m {
                Member::Method(md) => self.collect_method(&t.name, md),
                Member::Field(f) => self.unsupported(f.span, "a field"),
                Member::Property(p) => self.unsupported(p.span, "a property"),
                Member::Constructor(c) => self.unsupported(c.span, "a constructor"),
                Member::Destructor(d) => self.unsupported(d.span, "a destructor"),
                Member::Type(n) => self.collect_type(n),
                Member::Enum(e) => self.unsupported(e.span, "`enum`"),
            }
        }
    }

    fn collect_method(&mut self, class: &str, md: &'a MethodDecl) {
        if !md.modifiers.contains(Modifier::Static) {
            self.unsupported(md.span, "an instance method (objects)");
            return;
        }
        if !md.generics.is_empty() {
            self.unsupported(md.span, "a generic method");
            return;
        }
        if md.modifiers.contains(Modifier::Async) {
            self.unsupported(md.span, "`async`");
            return;
        }
        let mut params = Vec::new();
        for p in &md.params {
            match type_ct(&p.ty) {
                Some(ct) if ct != CT::Void => params.push((p.name.clone(), ct)),
                _ => {
                    self.unsupported(p.ty.span, "this parameter type");
                    return;
                }
            }
            if p.default.is_some() {
                self.unsupported(p.span, "a default argument");
            }
        }
        let ret = match &md.ret {
            None => CT::Void,
            Some(r) => match type_ct(r) {
                Some(ct) if ct != CT::StrArr => ct,
                _ => {
                    self.unsupported(r.span, "this return type");
                    return;
                }
            },
        };
        let c_name = format!("ioc_{}_{}_{}", self.methods.len(), sanitize(class), sanitize(&md.name));
        self.methods.push(Method { class: class.to_string(), name: md.name.clone(), c_name, params, ret, decl: md, file: self.file });
    }

    // ------------------------------------------------------- output

    fn signature(&self, i: usize) -> String {
        let m = &self.methods[i];
        let params: Vec<String> = m.params.iter().map(|(n, t)| format!("{} v_{}", t.c(), n)).collect();
        let ps = if params.is_empty() { "void".to_string() } else { params.join(", ") };
        format!("{} {}({})", m.ret.c(), m.c_name, ps)
    }

    fn line(&mut self, s: &str) {
        for _ in 0..self.ind {
            self.out.push_str("    ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn emit_method(&mut self, i: usize) {
        let (file, class, ret, decl) = {
            let m = &self.methods[i];
            (m.file, m.class.clone(), m.ret, m.decl)
        };
        self.file = file;
        self.class = class;
        self.ret = ret;
        self.scopes = vec![self.methods[i].params.iter().map(|(n, t)| (n.clone(), *t)).collect()];
        let sig = self.signature(i);
        self.line(&format!("static {} {{", sig));
        self.ind += 1;
        match &decl.body {
            Body::None => self.unsupported(decl.span, "a method without a body"),
            Body::Block(b) => {
                for s in &b.stmts {
                    self.stmt(s);
                }
            }
            Body::Expr(e) => {
                if let Some((c, _)) = self.expr(e) {
                    if ret == CT::Void {
                        self.line(&format!("{};", c));
                    } else {
                        self.line(&format!("return {};", c));
                    }
                }
            }
        }
        self.ind -= 1;
        self.line("}");
        self.out.push('\n');
    }

    // ------------------------------------------------------- statements

    fn lookup(&self, name: &str) -> Option<CT> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Local { name, ty, init, .. } => {
                let declared = match ty {
                    Some(t) => match type_ct(t) {
                        Some(ct) => Some(ct),
                        None => {
                            self.unsupported(t.span, "this type");
                            return;
                        }
                    },
                    None => None,
                };
                let (init_c, init_t) = match init {
                    Some(e) => match self.expr(e) {
                        Some((c, t)) => (Some(c), Some(t)),
                        None => return,
                    },
                    None => (None, None),
                };
                let Some(ct) = declared.or(init_t) else {
                    self.err(s.span, "cannot determine the type of this variable");
                    return;
                };
                match init_c {
                    Some(c) => self.line(&format!("{} v_{} = {};", ct.c(), name, c)),
                    None => self.line(&format!("{} v_{};", ct.c(), name)),
                }
                self.scopes.last_mut().unwrap().insert(name.clone(), ct);
            }
            StmtKind::Expr(e) => {
                if let Some((c, _)) = self.expr(e) {
                    self.line(&format!("{};", c));
                }
            }
            StmtKind::Block(b) => {
                self.line("{");
                self.block_body(b);
                self.line("}");
            }
            StmtKind::Unsafe(b) => self.unsupported(b.span, "`unsafe`"),
            StmtKind::If { cond, then, otherwise } => {
                let c = self.expr(cond).map(|x| x.0).unwrap_or_default();
                self.line(&format!("if ({}) {{", c));
                self.sub(then);
                if let Some(o) = otherwise {
                    self.line("} else {");
                    self.sub(o);
                }
                self.line("}");
            }
            StmtKind::While { cond, body } => {
                let c = self.expr(cond).map(|x| x.0).unwrap_or_default();
                self.line(&format!("while ({}) {{", c));
                self.sub(body);
                self.line("}");
            }
            StmtKind::For { init, cond, update, body } => {
                self.line("{");
                self.ind += 1;
                self.scopes.push(HashMap::new());
                if let Some(i) = init {
                    self.stmt(i);
                }
                let c = match cond {
                    Some(c) => self.expr(c).map(|x| x.0).unwrap_or_default(),
                    None => String::new(),
                };
                let u = match update {
                    Some(u) => self.expr(u).map(|x| x.0).unwrap_or_default(),
                    None => String::new(),
                };
                self.line(&format!("for (; {}; {}) {{", c, u));
                self.sub(body);
                self.line("}");
                self.scopes.pop();
                self.ind -= 1;
                self.line("}");
            }
            StmtKind::ForIn { iter, .. } => self.unsupported(iter.span, "`for ... in`"),
            StmtKind::Return(v) => match v {
                Some(e) => {
                    if let Some((c, _)) = self.expr(e) {
                        self.line(&format!("return {};", c));
                    }
                }
                None => self.line("return;"),
            },
            StmtKind::Break => self.line("break;"),
            StmtKind::Continue => self.line("continue;"),
        }
    }

    fn block_body(&mut self, b: &Block) {
        self.ind += 1;
        self.scopes.push(HashMap::new());
        for s in &b.stmts {
            self.stmt(s);
        }
        self.scopes.pop();
        self.ind -= 1;
    }

    fn sub(&mut self, s: &Stmt) {
        self.ind += 1;
        self.scopes.push(HashMap::new());
        match &s.kind {
            StmtKind::Block(b) => {
                for x in &b.stmts {
                    self.stmt(x);
                }
            }
            _ => self.stmt(s),
        }
        self.scopes.pop();
        self.ind -= 1;
    }

    // ------------------------------------------------------- expressions

    fn to_str(&mut self, c: String, t: CT, span: Span) -> Option<String> {
        Some(match t {
            CT::Str => c,
            CT::Bool => format!("ioc_str_bool({})", c),
            CT::Char => format!("ioc_str_char({})", c),
            CT::F32 | CT::F64 => format!("ioc_str_f64({})", c),
            CT::U64 | CT::U32 | CT::U16 | CT::U8 => format!("ioc_str_u64({})", c),
            CT::I8 | CT::I16 | CT::I32 | CT::I64 => format!("ioc_str_i64({})", c),
            _ => {
                self.unsupported(span, "converting this value to text");
                return None;
            }
        })
    }

    fn cat(parts: Vec<String>) -> String {
        format!("ioc_cat({}, (ioc_string[]){{{}}})", parts.len(), parts.join(", "))
    }

    fn expr(&mut self, e: &Expr) -> Option<(String, CT)> {
        match &e.kind {
            ExprKind::Int { text, suffix } => {
                let clean: String = text.chars().filter(|c| *c != '_').collect();
                let sfx = suffix.to_ascii_lowercase();
                let (lit, ct) = match sfx.as_str() {
                    "l" => (format!("{}LL", clean), CT::I64),
                    "u" => (format!("{}U", clean), CT::U32),
                    "ul" | "lu" => (format!("{}ULL", clean), CT::U64),
                    _ => {
                        let v = parse_int(&clean);
                        match v {
                            Some(v) if v <= i32::MAX as u128 => (clean.clone(), CT::I32),
                            Some(_) => (format!("{}LL", clean), CT::I64),
                            None => {
                                self.err(e.span, "this number is too large");
                                return None;
                            }
                        }
                    }
                };
                Some((format!("(({}){})", ct.c(), lit), ct))
            }
            ExprKind::Float { text, suffix } => {
                let clean: String = text.chars().filter(|c| *c != '_').collect();
                if suffix.eq_ignore_ascii_case("f") {
                    Some((format!("{}f", clean), CT::F32))
                } else {
                    Some((format!("(double){}", clean), CT::F64))
                }
            }
            ExprKind::Str(s) => Some((c_string(s), CT::Str)),
            ExprKind::Char(c) => Some((format!("((uint32_t){})", *c as u32), CT::Char)),
            ExprKind::Bool(b) => Some((b.to_string(), CT::Bool)),
            ExprKind::Interp(segs) => {
                let mut parts = Vec::new();
                for s in segs {
                    match s {
                        InterpSegment::Text(t) => parts.push(c_string(t)),
                        InterpSegment::Expr(x) => {
                            let (c, t) = self.expr(x)?;
                            parts.push(self.to_str(c, t, x.span)?);
                        }
                    }
                }
                if parts.is_empty() {
                    return Some(("\"\"".into(), CT::Str));
                }
                Some((Self::cat(parts), CT::Str))
            }
            ExprKind::Ident(name) => match self.lookup(name) {
                Some(t) => Some((format!("v_{}", name), t)),
                None => {
                    self.unsupported(e.span, &format!("using `{}` (a field or static member)", name));
                    None
                }
            },
            ExprKind::Member { target, name } => {
                let (c, t) = self.expr(target)?;
                match (t, name.as_str()) {
                    (CT::Str, "length") => Some((format!("ioc_strlen({})", c), CT::I32)),
                    (CT::StrArr, "length") => Some((format!("((int32_t)({}).length)", c), CT::I32)),
                    _ => {
                        self.unsupported(e.span, &format!("the member `{}`", name));
                        None
                    }
                }
            }
            ExprKind::Index { target, index } => {
                let (c, t) = self.expr(target)?;
                let (i, it) = self.expr(index)?;
                if t != CT::StrArr || !it.is_int() {
                    self.unsupported(e.span, "indexing this value");
                    return None;
                }
                Some((format!("({}).items[IOC_INDEX({}, ({}).length)]", c, i, c), CT::Str))
            }
            ExprKind::Call { callee, args } => self.call(callee, args, e.span),
            ExprKind::Unary { op, expr } => {
                let (c, t) = self.expr(expr)?;
                match op {
                    UnOp::Not => Some((format!("(!{})", c), CT::Bool)),
                    UnOp::Neg => {
                        let r = if t.is_float() { t } else { promote(t, CT::I32) };
                        Some((format!("(-({}){})", r.c(), c), r))
                    }
                }
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, e.span),
            ExprKind::Assign { op, target, value } => {
                let ExprKind::Ident(name) = &target.kind else {
                    self.unsupported(target.span, "assigning to a field or element");
                    return None;
                };
                let Some(tt) = self.lookup(name) else {
                    self.unsupported(target.span, &format!("assigning to `{}`", name));
                    return None;
                };
                let (v, vt) = self.expr(value)?;
                let var = format!("v_{}", name);
                let c = match op {
                    AssignOp::Assign => format!("({} = {})", var, v),
                    AssignOp::Add if tt == CT::Str => {
                        let s = self.to_str(v, vt, value.span)?;
                        format!("({} = {})", var, Self::cat(vec![var.clone(), s]))
                    }
                    AssignOp::Div | AssignOp::Rem if tt.is_int() => {
                        let f = arith_fn(*op == AssignOp::Div, tt.is_unsigned() || vt.is_unsigned());
                        format!("({} = ({}){}({}, {}))", var, tt.c(), f, var, v)
                    }
                    AssignOp::Rem => format!("({} = fmod({}, {}))", var, var, v),
                    other => format!("({} {} {})", var, other.as_str(), v),
                };
                Some((c, tt))
            }
            ExprKind::Postfix { op, expr } => {
                let ExprKind::Ident(name) = &expr.kind else {
                    self.unsupported(expr.span, "`++`/`--` on a field or element");
                    return None;
                };
                let t = self.lookup(name)?;
                let o = if *op == PostOp::Inc { "++" } else { "--" };
                Some((format!("(v_{}{})", name, o), t))
            }
            ExprKind::This => {
                self.unsupported(e.span, "`this`");
                None
            }
            ExprKind::Base => {
                self.unsupported(e.span, "`base`");
                None
            }
            ExprKind::New { .. } => {
                self.unsupported(e.span, "`new` (objects)");
                None
            }
            ExprKind::Try(_) => {
                self.unsupported(e.span, "the `?` operator (Result/Option)");
                None
            }
            ExprKind::Await(_) => {
                self.unsupported(e.span, "`await`");
                None
            }
            ExprKind::Borrow { .. } => {
                self.unsupported(e.span, "`ref` / `mut ref`");
                None
            }
            ExprKind::Match { .. } => {
                self.unsupported(e.span, "`match`");
                None
            }
            ExprKind::Spawn(_) => {
                self.unsupported(e.span, "`spawn`");
                None
            }
            ExprKind::Block(_) | ExprKind::Return(_) => {
                self.unsupported(e.span, "a block or `return` used as a value");
                None
            }
            ExprKind::Array(_) => {
                self.unsupported(e.span, "an array literal");
                None
            }
        }
    }

    fn binary(&mut self, op: BinOp, l: &Expr, r: &Expr, span: Span) -> Option<(String, CT)> {
        let (lc, lt) = self.expr(l)?;
        let (rc, rt) = self.expr(r)?;
        match op {
            BinOp::And => Some((format!("({} && {})", lc, rc), CT::Bool)),
            BinOp::Or => Some((format!("({} || {})", lc, rc), CT::Bool)),
            BinOp::Coalesce => {
                self.unsupported(span, "`??`");
                None
            }
            BinOp::Add if lt == CT::Str || rt == CT::Str => {
                let a = self.to_str(lc, lt, l.span)?;
                let b = self.to_str(rc, rt, r.span)?;
                Some((Self::cat(vec![a, b]), CT::Str))
            }
            BinOp::Eq | BinOp::Ne if lt == CT::Str && rt == CT::Str => {
                let c = format!("ioc_streq({}, {})", lc, rc);
                Some((if op == BinOp::Eq { c } else { format!("(!{})", c) }, CT::Bool))
            }
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge if lt == CT::Str && rt == CT::Str => {
                Some((format!("(ioc_strcmp({}, {}) {} 0)", lc, rc, op.as_str()), CT::Bool))
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                Some((format!("({} {} {})", lc, op.as_str(), rc), CT::Bool))
            }
            _ => {
                if !(lt.is_num() && rt.is_num()) {
                    self.unsupported(span, "this operator on these types");
                    return None;
                }
                let t = promote(lt, rt);
                match op {
                    BinOp::Div | BinOp::Rem if t.is_int() => {
                        let f = arith_fn(op == BinOp::Div, t.is_unsigned());
                        Some((format!("(({}){}((({}){}), (({}){})))", t.c(), f, wide(t), lc, wide(t), rc), t))
                    }
                    BinOp::Rem => Some((format!("fmod({}, {})", lc, rc), t)),
                    _ => Some((format!("(({}){} {} ({}){})", t.c(), lc, op.as_str(), t.c(), rc), t)),
                }
            }
        }
    }

    fn call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> Option<(String, CT)> {
        // evaluate the arguments first; their types pick the overload
        let mut av: Vec<(String, CT, Span)> = Vec::new();
        let mut failed = false;
        for a in args {
            match self.expr(a) {
                Some((c, t)) => av.push((c, t, a.span)),
                None => failed = true,
            }
        }
        if failed {
            return None;
        }
        match &callee.kind {
            ExprKind::Ident(name) => {
                let class = self.class.clone();
                self.user_call(&class, name, av, span)
            }
            ExprKind::Member { target, name } => {
                if let ExprKind::Ident(q) = &target.kind {
                    if self.lookup(q).is_none() {
                        return match q.as_str() {
                            "Console" => self.console(name, av, span),
                            "Math" => self.math(name, av, span),
                            _ => self.user_call(q, name, av, span),
                        };
                    }
                }
                let (tc, tt) = self.expr(target)?;
                self.value_method(tc, tt, name, av, span)
            }
            _ => {
                self.unsupported(span, "calling this kind of expression");
                None
            }
        }
    }

    fn user_call(&mut self, class: &str, name: &str, av: Vec<(String, CT, Span)>, span: Span) -> Option<(String, CT)> {
        let mut best: Option<(usize, usize)> = None; // (score, method index)
        for (i, m) in self.methods.iter().enumerate() {
            if m.class != class || m.name != name || m.params.len() != av.len() {
                continue;
            }
            let mut score = 0;
            let mut ok = true;
            for ((_, pt), (_, at, _)) in m.params.iter().zip(av.iter()) {
                if pt == at {
                    score += 2;
                } else if pt.is_num() && at.is_num() && (pt.is_float() || !at.is_float()) {
                    score += 1;
                } else {
                    ok = false;
                }
            }
            if ok && best.map_or(true, |(s, _)| score > s) {
                best = Some((score, i));
            }
        }
        let Some((_, i)) = best else {
            self.err(span, format!("the backend found no method `{}.{}` taking these arguments", class, name));
            return None;
        };
        let m = &self.methods[i];
        let ret = m.ret;
        let c_name = m.c_name.clone();
        let list: Vec<String> = av.into_iter().map(|(c, _, _)| c).collect();
        Some((format!("{}({})", c_name, list.join(", ")), ret))
    }

    fn console(&mut self, name: &str, mut av: Vec<(String, CT, Span)>, span: Span) -> Option<(String, CT)> {
        let f = match name {
            "println" => "ioc_println",
            "print" => "ioc_print",
            "error" => "ioc_eprintln",
            _ => {
                self.unsupported(span, &format!("`Console.{}`", name));
                return None;
            }
        };
        if av.len() != 1 || av[0].1 != CT::Str {
            self.unsupported(span, "this use of `Console`");
            return None;
        }
        let (c, _, _) = av.remove(0);
        Some((format!("{}({})", f, c), CT::Void))
    }

    fn math(&mut self, name: &str, av: Vec<(String, CT, Span)>, span: Span) -> Option<(String, CT)> {
        let any_float = av.iter().any(|a| a.1.is_float());
        let cs: Vec<String> = av.iter().map(|a| a.0.clone()).collect();
        let (f, ret): (String, CT) = match (name, av.len()) {
            ("abs", 1) if any_float => ("fabs".into(), CT::F64),
            ("abs", 1) => ("ioc_abs_i".into(), CT::I32),
            ("min", 2) if any_float => ("ioc_min_d".into(), CT::F64),
            ("min", 2) => ("ioc_min_i".into(), CT::I32),
            ("max", 2) if any_float => ("ioc_max_d".into(), CT::F64),
            ("max", 2) => ("ioc_max_i".into(), CT::I32),
            ("sqrt", 1) => ("sqrt".into(), CT::F64),
            ("pow", 2) => ("pow".into(), CT::F64),
            ("floor", 1) => ("floor".into(), CT::F64),
            ("ceil", 1) => ("ceil".into(), CT::F64),
            ("round", 1) => ("round".into(), CT::F64),
            _ => {
                self.unsupported(span, &format!("`Math.{}`", name));
                return None;
            }
        };
        let call = format!("{}({})", f, cs.join(", "));
        Some((if ret == CT::I32 { format!("((int32_t){})", call) } else { call }, ret))
    }

    fn value_method(&mut self, tc: String, tt: CT, name: &str, av: Vec<(String, CT, Span)>, span: Span) -> Option<(String, CT)> {
        let a: Vec<String> = av.iter().map(|x| x.0.clone()).collect();
        match (tt, name, a.len()) {
            (t, "toString", 0) if t != CT::StrArr && t != CT::Void => {
                let s = self.to_str(tc, t, span)?;
                Some((s, CT::Str))
            }
            (CT::Str, "isEmpty", 0) => Some((format!("({}[0] == 0)", tc), CT::Bool)),
            (CT::Str, "contains", 1) => Some((format!("ioc_contains({}, {})", tc, a[0]), CT::Bool)),
            (CT::Str, "startsWith", 1) => Some((format!("ioc_starts({}, {})", tc, a[0]), CT::Bool)),
            (CT::Str, "endsWith", 1) => Some((format!("ioc_ends({}, {})", tc, a[0]), CT::Bool)),
            (CT::Str, "toUpper", 0) => Some((format!("ioc_upper({})", tc), CT::Str)),
            (CT::Str, "toLower", 0) => Some((format!("ioc_lower({})", tc), CT::Str)),
            (CT::Str, "trim", 0) => Some((format!("ioc_trim({})", tc), CT::Str)),
            (CT::Str, "substring", 2) => Some((format!("ioc_substring({}, {}, {})", tc, a[0], a[1]), CT::Str)),
            _ => {
                self.unsupported(span, &format!("the method `{}`", name));
                None
            }
        }
    }
}

fn parse_int(s: &str) -> Option<u128> {
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u128::from_str_radix(h, 16).ok()
    } else if let Some(b) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B")) {
        u128::from_str_radix(b, 2).ok()
    } else {
        s.parse().ok()
    }
}

fn arith_fn(div: bool, unsigned: bool) -> &'static str {
    match (div, unsigned) {
        (true, false) => "ioc_div",
        (false, false) => "ioc_rem",
        (true, true) => "ioc_udiv",
        (false, true) => "ioc_urem",
    }
}

fn wide(t: CT) -> &'static str {
    if t.is_unsigned() {
        "uint64_t"
    } else {
        "int64_t"
    }
}
