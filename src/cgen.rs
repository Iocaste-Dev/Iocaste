//! C backend (v0.0.5-alpha): turns a type-checked program into one C file.
//!
//! Supported so far: classes (fields, auto-properties, constructors, destructors, static and instance
//! methods with overloads, `new`, `this`), ownership (values are freed when their owner goes out of
//! scope; moving leaves NULL behind), `ref` / `mut ref` to objects; the primitive types, `string`,
//! `string[]` (command-line arguments); `let`/`var`; `if`/`while`/`for`; arithmetic; string
//! interpolation and `+`; `Console` and `Math`; a few `string` methods.
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
    /// an owned object (class index)
    Obj(usize),
    /// a borrowed object (`ref` / `mut ref`, or `this`)
    Ref(usize),
}

impl CT {
    fn c(self) -> String {
        match self {
            CT::Obj(i) | CT::Ref(i) => return format!("ioc_c{}*", i),
            _ => {}
        }
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
            CT::Obj(_) | CT::Ref(_) => unreachable!(),
        }
        .to_string()
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

#[allow(dead_code)]
struct Method<'a> {
    class: String,
    class_idx: usize,
    name: String,
    c_name: String,
    params: Vec<(String, CT)>,
    ret: CT,
    is_static: bool,
    body: &'a Body,
    span: Span,
    file: usize,
}

#[allow(dead_code)]
struct FieldI<'a> {
    name: String,
    ct: CT,
    init: Option<&'a Expr>,
    /// a property with a getter body: reading it calls this method
    getter: Option<usize>,
    /// writable from outside through assignment
    span: Span,
}

struct CtorI<'a> {
    params: Vec<(String, CT)>,
    decl: &'a CtorDecl,
    c_name: String,
}

#[allow(dead_code)]
struct Class<'a> {
    name: String,
    file: usize,
    span: Span,
    fields: Vec<FieldI<'a>>,
    ctors: Vec<CtorI<'a>>,
    dtor: Option<&'a DtorDecl>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Method,
    Ctor,
    Dtor,
}

#[derive(Default)]
struct Scope {
    vars: HashMap<String, CT>,
    /// owned objects declared here, freed (in reverse order) when the scope ends
    owned: Vec<String>,
}

enum ArgKind {
    Plain,
    /// a local variable holding an owned object: passing it by value moves it
    Local,
    /// a field or element: cannot be moved out
    Place,
}

struct Arg {
    code: String,
    ct: CT,
    span: Span,
    kind: ArgKind,
}

struct Gen<'a> {
    methods: Vec<Method<'a>>,
    classes: Vec<Class<'a>>,
    errors: Vec<CError>,
    file: usize,
    /// the class whose code is being generated
    class: Option<usize>,
    in_static: bool,
    ctx: Ctx,
    scopes: Vec<Scope>,
    loop_bases: Vec<usize>,
    ret: CT,
    out: String,
    ind: usize,
}

impl<'a> Gen<'a> {
    fn find_class(&self, name: &str) -> Option<usize> {
        self.classes.iter().position(|c| c.name == name)
    }

    fn type_ct(&self, t: &TypeRef) -> Option<CT> {
        match &t.kind {
            TypeRefKind::Named { path, args } if args.is_empty() => {
                if path.len() == 1 {
                    let p = match path[0].as_str() {
                        "bool" => Some(CT::Bool),
                        "char" => Some(CT::Char),
                        "sbyte" => Some(CT::I8),
                        "short" => Some(CT::I16),
                        "int" => Some(CT::I32),
                        "long" | "isize" => Some(CT::I64),
                        "byte" => Some(CT::U8),
                        "ushort" => Some(CT::U16),
                        "uint" => Some(CT::U32),
                        "ulong" | "usize" => Some(CT::U64),
                        "float" => Some(CT::F32),
                        "double" => Some(CT::F64),
                        "string" => Some(CT::Str),
                        "void" => Some(CT::Void),
                        _ => None,
                    };
                    if p.is_some() {
                        return p;
                    }
                }
                self.find_class(path.last()?).map(CT::Obj)
            }
            TypeRefKind::Array(inner) => match self.type_ct(inner) {
                Some(CT::Str) => Some(CT::StrArr),
                _ => None,
            },
            TypeRefKind::Ref { inner, .. } => match self.type_ct(inner) {
                Some(CT::Obj(i)) => Some(CT::Ref(i)),
                _ => None,
            },
            _ => None,
        }
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
        classes: Vec::new(),
        errors: Vec::new(),
        file: 0,
        class: None,
        in_static: true,
        ctx: Ctx::Method,
        scopes: Vec::new(),
        loop_bases: Vec::new(),
        ret: CT::Void,
        out: String::new(),
        ind: 0,
    };
    // pass 1: class names (so types can refer to each other in any order)
    for (i, f) in files.iter().enumerate() {
        g.file = i;
        g.register_items(&f.items);
    }
    // pass 2: members
    let mut next = 0;
    for (i, f) in files.iter().enumerate() {
        g.file = i;
        g.collect_items(&f.items, &mut next);
    }
    if !g.errors.is_empty() {
        return Err(g.errors);
    }
    let entries: Vec<usize> =
        g.methods.iter().enumerate().filter(|(_, m)| m.name == "main" && m.is_static).map(|(i, _)| i).collect();
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
            diag: Diagnostic::new("there is more than one `main` method", m.span).with_hint("a program has exactly one entry point"),
        }]);
    }
    let e = &g.methods[entry];
    let bad_entry = !(matches!(e.ret, CT::I32 | CT::Void)
        && (e.params.is_empty() || (e.params.len() == 1 && e.params[0].1 == CT::StrArr)));
    if bad_entry {
        return Err(vec![CError {
            file: e.file,
            diag: Diagnostic::new("`main` must be `main(): int`, `main(args: string[]): int` (or return `void`)", e.span),
        }]);
    }

    g.out.push_str(RUNTIME);
    g.out.push_str("\n/* ---- program */\n");
    for i in 0..g.classes.len() {
        g.out.push_str(&format!("typedef struct ioc_c{0} ioc_c{0};\n", i));
    }
    for i in 0..g.classes.len() {
        g.emit_struct(i);
    }
    for i in 0..g.classes.len() {
        g.out.push_str(&format!("static void ioc_drop_{0}(ioc_c{0} *v_this);\n", i));
        for k in 0..g.classes[i].ctors.len() {
            let sig = g.ctor_signature(i, k);
            g.out.push_str(&format!("static {};\n", sig));
        }
        if g.classes[i].ctors.is_empty() {
            g.out.push_str(&format!("static ioc_c{0} *ioc_new_{0}_default(void);\n", i));
        }
    }
    for i in 0..g.methods.len() {
        let sig = g.signature(i);
        g.out.push_str(&format!("static {};\n", sig));
    }
    g.out.push('\n');
    for i in 0..g.classes.len() {
        g.emit_class(i);
    }
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
            diag: Diagnostic::new(format!("{} is not supported by the C backend yet (v0.0.7)", what), span)
                .with_hint("`ioc check` still validates it; code generation for it arrives in the next versions"),
        });
    }

    // ------------------------------------------------------- collecting

    fn register_items(&mut self, items: &'a [Item]) {
        for it in items {
            match it {
                Item::Namespace(n) => self.register_items(&n.items),
                Item::Type(t) => self.register_type(t),
                _ => {}
            }
        }
    }

    fn register_type(&mut self, t: &'a TypeDecl) {
        if t.kind == TypeKind::Class && t.generics.is_empty() {
            self.classes.push(Class { name: t.name.clone(), file: self.file, span: t.span, fields: Vec::new(), ctors: Vec::new(), dtor: None });
        }
    }

    fn collect_items(&mut self, items: &'a [Item], next_class: &mut usize) {
        for it in items {
            match it {
                Item::Namespace(n) => self.collect_items(&n.items, next_class),
                Item::Import(_) => {}
                Item::Enum(e) => self.unsupported(e.span, "`enum`"),
                Item::Type(t) => self.collect_type(t, next_class),
            }
        }
    }

    fn collect_type(&mut self, t: &'a TypeDecl, next_class: &mut usize) {
        if t.kind != TypeKind::Class {
            self.unsupported(
                t.span,
                &format!(
                    "`{}`",
                    match t.kind {
                        TypeKind::Struct => "struct",
                        TypeKind::Record => "record",
                        _ => "interface",
                    }
                ),
            );
            return;
        }
        if !t.generics.is_empty() {
            self.unsupported(t.span, "a generic class");
            return;
        }
        if !t.bases.is_empty() {
            self.unsupported(t.span, "inheritance and interfaces");
        }
        let ci = *next_class;
        *next_class += 1;
        for m in &t.members {
            match m {
                Member::Method(md) => self.collect_method(ci, &t.name, md),
                Member::Field(f) => {
                    if f.modifiers.contains(Modifier::Static) {
                        self.unsupported(f.span, "a static field");
                        continue;
                    }
                    match self.type_ct(&f.ty) {
                        Some(ct) if ct != CT::Void && !matches!(ct, CT::Ref(_) | CT::StrArr) => {
                            self.classes[ci].fields.push(FieldI { name: f.name.clone(), ct, init: f.init.as_ref(), getter: None, span: f.span })
                        }
                        _ => self.unsupported(f.ty.span, "this field type"),
                    }
                }
                Member::Property(p) => self.collect_property(ci, &t.name, p),
                Member::Constructor(c) => {
                    let mut params = Vec::new();
                    let mut ok = true;
                    for p in &c.params {
                        match self.type_ct(&p.ty) {
                            Some(ct) if ct != CT::Void => params.push((p.name.clone(), ct)),
                            _ => {
                                self.unsupported(p.ty.span, "this parameter type");
                                ok = false;
                            }
                        }
                        if p.default.is_some() {
                            self.unsupported(p.span, "a default argument");
                        }
                    }
                    if ok {
                        let k = self.classes[ci].ctors.len();
                        self.classes[ci].ctors.push(CtorI { params, decl: c, c_name: format!("ioc_new_{}_{}", ci, k) });
                    }
                }
                Member::Destructor(d) => self.classes[ci].dtor = Some(d),
                Member::Type(n) => self.unsupported(n.span, "a nested type"),
                Member::Enum(e) => self.unsupported(e.span, "`enum`"),
            }
        }
    }

    fn collect_property(&mut self, ci: usize, class: &str, p: &'a PropertyDecl) {
        if p.modifiers.contains(Modifier::Static) {
            self.unsupported(p.span, "a static property");
            return;
        }
        let Some(ct) = self.type_ct(&p.ty).filter(|c| *c != CT::Void && !matches!(c, CT::Ref(_) | CT::StrArr)) else {
            self.unsupported(p.ty.span, "this property type");
            return;
        };
        let get = p.accessors.iter().find(|a| a.kind == AccessorKind::Get);
        let set = p.accessors.iter().find(|a| a.kind == AccessorKind::Set);
        let auto_get = matches!(get.map(|a| &a.body), Some(Body::None));
        let auto_set = set.map_or(true, |a| matches!(a.body, Body::None));
        if auto_get && auto_set {
            // stored in the object like a field
            self.classes[ci].fields.push(FieldI { name: p.name.clone(), ct, init: p.init.as_ref(), getter: None, span: p.span });
            return;
        }
        if set.is_some() {
            self.unsupported(p.span, "a property with a custom setter");
            return;
        }
        let Some(get) = get else {
            self.unsupported(p.span, "a property without a getter");
            return;
        };
        let k = self.methods.len();
        let c_name = format!("ioc_{}_{}_get_{}", k, sanitize(class), sanitize(&p.name));
        self.methods.push(Method {
            class: class.to_string(),
            class_idx: ci,
            name: format!("get_{}", p.name),
            c_name,
            params: Vec::new(),
            ret: ct,
            is_static: false,
            body: &get.body,
            span: p.span,
            file: self.file,
        });
        self.classes[ci].fields.push(FieldI { name: p.name.clone(), ct, init: None, getter: Some(k), span: p.span });
    }

    fn collect_method(&mut self, ci: usize, class: &str, md: &'a MethodDecl) {
        if !md.generics.is_empty() {
            self.unsupported(md.span, "a generic method");
            return;
        }
        if md.modifiers.contains(Modifier::Async) {
            self.unsupported(md.span, "`async`");
            return;
        }
        if md.modifiers.contains(Modifier::Virtual) || md.modifiers.contains(Modifier::Override) || md.modifiers.contains(Modifier::Abstract) {
            self.unsupported(md.span, "`virtual` / `override` / `abstract`");
            return;
        }
        let mut params = Vec::new();
        for p in &md.params {
            match self.type_ct(&p.ty) {
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
            Some(r) => match self.type_ct(r) {
                Some(ct) if ct != CT::StrArr => ct,
                _ => {
                    self.unsupported(r.span, "this return type");
                    return;
                }
            },
        };
        let c_name = format!("ioc_{}_{}_{}", self.methods.len(), sanitize(class), sanitize(&md.name));
        self.methods.push(Method {
            class: class.to_string(),
            class_idx: ci,
            name: md.name.clone(),
            c_name,
            params,
            ret,
            is_static: md.modifiers.contains(Modifier::Static),
            body: &md.body,
            span: md.span,
            file: self.file,
        });
    }

    // ------------------------------------------------------- output

    fn emit_struct(&mut self, i: usize) {
        let mut s = format!("struct ioc_c{} {{\n", i);
        let c = &self.classes[i];
        let mut any = false;
        for f in &c.fields {
            if f.getter.is_none() {
                s.push_str(&format!("    {} f_{};\n", f.ct.c(), f.name));
                any = true;
            }
        }
        if !any {
            s.push_str("    char pad_;\n");
        }
        s.push_str("};\n");
        self.out.push_str(&s);
    }

    fn signature(&self, i: usize) -> String {
        let m = &self.methods[i];
        let mut params: Vec<String> = Vec::new();
        if !m.is_static {
            params.push(format!("ioc_c{}* v_this", m.class_idx));
        }
        params.extend(m.params.iter().map(|(n, t)| format!("{} v_{}", t.c(), n)));
        let ps = if params.is_empty() { "void".to_string() } else { params.join(", ") };
        format!("{} {}({})", m.ret.c(), m.c_name, ps)
    }

    fn ctor_signature(&self, ci: usize, k: usize) -> String {
        let c = &self.classes[ci].ctors[k];
        let params: Vec<String> = c.params.iter().map(|(n, t)| format!("{} v_{}", t.c(), n)).collect();
        let ps = if params.is_empty() { "void".to_string() } else { params.join(", ") };
        format!("ioc_c{}* {}({})", ci, c.c_name, ps)
    }

    fn line(&mut self, s: &str) {
        for _ in 0..self.ind {
            self.out.push_str("    ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    // ---- scopes and ownership

    fn push_scope(&mut self) {
        self.scopes.push(Scope::default());
    }

    fn drop_lines(&mut self, from: usize) {
        let mut lines = Vec::new();
        for sc in self.scopes[from..].iter().rev() {
            for n in sc.owned.iter().rev() {
                if let Some(CT::Obj(i)) = sc.vars.get(n) {
                    lines.push(format!("ioc_drop_{}(v_{});", i, n));
                }
            }
        }
        for l in lines {
            self.line(&l);
        }
    }

    fn pop_scope(&mut self) {
        let from = self.scopes.len() - 1;
        self.drop_lines(from);
        self.scopes.pop();
    }

    fn declare(&mut self, name: &str, ct: CT) {
        let sc = self.scopes.last_mut().unwrap();
        sc.vars.insert(name.to_string(), ct);
        if matches!(ct, CT::Obj(_)) {
            sc.owned.retain(|n| n != name);
            sc.owned.push(name.to_string());
        }
    }

    fn lookup(&self, name: &str) -> Option<CT> {
        self.scopes.iter().rev().find_map(|s| s.vars.get(name).copied())
    }

    fn emit_return(&mut self, val: Option<(String, CT)>) {
        let ctor = self.ctx == Ctx::Ctor;
        match val {
            Some((c, t)) if t != CT::Void => {
                self.line("{");
                self.ind += 1;
                self.line(&format!("{} ioc_r = {};", t.c(), c));
                self.drop_lines(0);
                self.line("return ioc_r;");
                self.ind -= 1;
                self.line("}");
            }
            _ => {
                self.line("{");
                self.ind += 1;
                if self.ctx == Ctx::Dtor {
                    self.line("goto ioc_dtor_end;");
                } else {
                    self.drop_lines(0);
                    self.line(if ctor { "return v_this;" } else { "return;" });
                }
                self.ind -= 1;
                self.line("}");
            }
        }
    }

    // ---- classes

    fn emit_class(&mut self, ci: usize) {
        self.file = self.classes[ci].file;
        self.class = Some(ci);
        // destructor + drop
        self.ctx = Ctx::Dtor;
        self.in_static = false;
        self.ret = CT::Void;
        self.scopes = vec![Scope::default()];
        self.line(&format!("static void ioc_drop_{0}(ioc_c{0} *v_this) {{", ci));
        self.ind += 1;
        self.line("if (!v_this) return;");
        if let Some(d) = self.classes[ci].dtor {
            self.line("{");
            self.ind += 1;
            self.push_scope();
            for s in &d.body.stmts {
                self.stmt(s);
            }
            self.pop_scope();
            self.ind -= 1;
            self.line("}");
            self.line("ioc_dtor_end:; (void)0;");
        }
        let owned: Vec<(String, usize)> = self.classes[ci]
            .fields
            .iter()
            .filter(|f| f.getter.is_none())
            .filter_map(|f| if let CT::Obj(j) = f.ct { Some((f.name.clone(), j)) } else { None })
            .collect();
        for (n, j) in owned {
            self.line(&format!("ioc_drop_{}(v_this->f_{});", j, n));
        }
        self.line("free(v_this);");
        self.ind -= 1;
        self.line("}");
        self.out.push('\n');

        // constructors
        self.ctx = Ctx::Ctor;
        if self.classes[ci].ctors.is_empty() {
            self.scopes = vec![Scope::default()];
            self.line(&format!("static ioc_c{0} *ioc_new_{0}_default(void) {{", ci));
            self.ind += 1;
            self.line(&format!("ioc_c{0} *v_this = (ioc_c{0} *)ioc_zalloc(sizeof(ioc_c{0}));", ci));
            self.emit_field_inits(ci);
            self.line("return v_this;");
            self.ind -= 1;
            self.line("}");
            self.out.push('\n');
        }
        for k in 0..self.classes[ci].ctors.len() {
            let sig = self.ctor_signature(ci, k);
            let (decl, params): (&'a CtorDecl, Vec<(String, CT)>) = {
                let c = &self.classes[ci].ctors[k];
                (c.decl, c.params.clone())
            };
            self.scopes = vec![Scope::default()];
            for (n, t) in &params {
                self.declare(n, *t);
            }
            self.line(&format!("static {} {{", sig));
            self.ind += 1;
            self.line(&format!("ioc_c{0} *v_this = (ioc_c{0} *)ioc_zalloc(sizeof(ioc_c{0}));", ci));
            self.emit_field_inits(ci);
            self.push_scope();
            for s in &decl.body.stmts {
                self.stmt(s);
            }
            self.pop_scope();
            self.drop_lines(0);
            self.line("return v_this;");
            self.ind -= 1;
            self.line("}");
            self.out.push('\n');
        }
        self.ctx = Ctx::Method;
        self.class = None;
    }

    fn emit_field_inits(&mut self, ci: usize) {
        let inits: Vec<(String, &'a Expr)> =
            self.classes[ci].fields.iter().filter_map(|f| f.init.map(|e| (f.name.clone(), e))).collect();
        for (n, e) in inits {
            if let Some((c, _)) = self.owned_value(e) {
                self.line(&format!("v_this->f_{} = {};", n, c));
            }
        }
    }

    fn emit_method(&mut self, i: usize) {
        let (file, ci, ret, body, span, is_static) = {
            let m = &self.methods[i];
            (m.file, m.class_idx, m.ret, m.body, m.span, m.is_static)
        };
        self.file = file;
        self.class = Some(ci);
        self.in_static = is_static;
        self.ctx = Ctx::Method;
        self.ret = ret;
        self.scopes = vec![Scope::default()];
        self.loop_bases.clear();
        let params = self.methods[i].params.clone();
        for (n, t) in &params {
            self.declare(n, *t);
        }
        let sig = self.signature(i);
        self.line(&format!("static {} {{", sig));
        self.ind += 1;
        match body {
            Body::None => self.unsupported(span, "a method without a body"),
            Body::Block(b) => {
                self.push_scope();
                for s in &b.stmts {
                    self.stmt(s);
                }
                self.pop_scope();
                self.drop_lines(0);
            }
            Body::Expr(e) => {
                if ret == CT::Void {
                    if let Some((c, t)) = self.expr(e) {
                        self.discard(c, t);
                    }
                    self.drop_lines(0);
                } else if let Some(v) = self.owned_value(e) {
                    if self.check_owned(ret, v.1, e.span).is_some() {
                        self.emit_return(Some(v));
                    }
                }
            }
        }
        self.ind -= 1;
        self.line("}");
        self.out.push('\n');
        self.class = None;
    }

    /// An expression statement whose value is not used: owned objects must still be freed.
    fn discard(&mut self, c: String, t: CT) {
        match t {
            CT::Obj(i) => self.line(&format!("ioc_drop_{}({});", i, c)),
            _ => self.line(&format!("{};", c)),
        }
    }

    // ------------------------------------------------------- statements

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Local { name, ty, init, .. } => {
                let declared = match ty {
                    Some(t) => match self.type_ct(t) {
                        Some(ct) => Some(ct),
                        None => {
                            self.unsupported(t.span, "this type");
                            return;
                        }
                    },
                    None => None,
                };
                let (init_c, init_t) = match init {
                    Some(e) => match self.owned_value(e) {
                        Some((c, t)) => (Some(c), Some(t)),
                        None => return,
                    },
                    None => (None, None),
                };
                let Some(ct) = declared.or(init_t) else {
                    self.err(s.span, "cannot determine the type of this variable");
                    return;
                };
                if let Some(it) = init_t {
                    if self.check_owned(ct, it, s.span).is_none() {
                        return;
                    }
                }
                match init_c {
                    Some(c) => self.line(&format!("{} v_{} = {};", ct.c(), name, c)),
                    None if matches!(ct, CT::Obj(_)) => self.line(&format!("{} v_{} = NULL;", ct.c(), name)),
                    None => self.line(&format!("{} v_{};", ct.c(), name)),
                }
                self.declare(name, ct);
            }
            StmtKind::Expr(e) => {
                if let Some((c, t)) = self.expr(e) {
                    self.discard(c, t);
                }
            }
            StmtKind::Block(b) => {
                self.line("{");
                self.ind += 1;
                self.push_scope();
                for x in &b.stmts {
                    self.stmt(x);
                }
                self.pop_scope();
                self.ind -= 1;
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
                self.loop_bases.push(self.scopes.len());
                self.sub(body);
                self.loop_bases.pop();
                self.line("}");
            }
            StmtKind::For { init, cond, update, body } => {
                self.line("{");
                self.ind += 1;
                self.push_scope();
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
                self.loop_bases.push(self.scopes.len());
                self.sub(body);
                self.loop_bases.pop();
                self.line("}");
                self.pop_scope();
                self.ind -= 1;
                self.line("}");
            }
            StmtKind::ForIn { iter, .. } => self.unsupported(iter.span, "`for ... in`"),
            StmtKind::Return(v) => match v {
                Some(e) => {
                    if let Some(val) = self.owned_value(e) {
                        if self.check_owned(self.ret, val.1, e.span).is_none() {
                            return;
                        }
                        self.emit_return(Some(val));
                    }
                }
                None => self.emit_return(None),
            },
            StmtKind::Break | StmtKind::Continue => {
                let base = self.loop_bases.last().copied().unwrap_or(0);
                self.line("{");
                self.ind += 1;
                self.drop_lines(base);
                self.line(if matches!(s.kind, StmtKind::Break) { "break;" } else { "continue;" });
                self.ind -= 1;
                self.line("}");
            }
        }
    }

    fn sub(&mut self, s: &Stmt) {
        self.ind += 1;
        self.push_scope();
        match &s.kind {
            StmtKind::Block(b) => {
                for x in &b.stmts {
                    self.stmt(x);
                }
            }
            _ => self.stmt(s),
        }
        self.pop_scope();
        self.ind -= 1;
    }

    // ------------------------------------------------------- expressions

    /// The value is given away (stored, returned, passed by value): a local object is moved out.
    fn owned_value(&mut self, e: &Expr) -> Option<(String, CT)> {
        let (c, t) = self.expr(e)?;
        if matches!(t, CT::Obj(_)) {
            match &e.kind {
                ExprKind::Ident(n) if self.lookup(n).is_some() => return Some((format!("IOC_TAKE({})", c), t)),
                ExprKind::Ident(_) | ExprKind::Member { .. } | ExprKind::Index { .. } | ExprKind::This => {
                    self.unsupported(e.span, "moving an object out of a field (borrow it with `ref` instead)");
                    return None;
                }
                _ => {}
            }
        }
        Some((c, t))
    }

    /// An owned slot must not receive a borrowed object (`this`, `ref x`): it would be freed twice.
    fn check_owned(&mut self, target: CT, val: CT, span: Span) -> Option<()> {
        if matches!(target, CT::Obj(_)) && matches!(val, CT::Ref(_)) {
            self.err(span, "a borrowed object cannot be stored or returned as an owned one; give the receiver the type `ref ...`");
            return None;
        }
        Some(())
    }

    fn arg_for(&mut self, a: &Arg, param: CT) -> Option<String> {
        if matches!(param, CT::Obj(_)) {
            return match a.kind {
                ArgKind::Local => Some(format!("IOC_TAKE({})", a.code)),
                ArgKind::Place => {
                    self.unsupported(a.span, "moving an object out of a field (borrow it with `ref` instead)");
                    None
                }
                ArgKind::Plain => Some(a.code.clone()),
            };
        }
        Some(a.code.clone())
    }

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

    /// Field (or property) `name` of class `ci`, read through `obj`.
    fn field_of(&mut self, obj: String, ci: usize, name: &str, span: Span) -> Option<(String, CT)> {
        let Some(f) = self.classes[ci].fields.iter().find(|f| f.name == name) else {
            self.unsupported(span, &format!("the member `{}`", name));
            return None;
        };
        let ct = f.ct;
        match f.getter {
            Some(k) => Some((format!("{}({})", self.methods[k].c_name, obj), ct)),
            None => Some((format!("(({})->f_{})", obj, name), ct)),
        }
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
                    _ => match parse_int(&clean) {
                        Some(v) if v <= i32::MAX as u128 => (clean.clone(), CT::I32),
                        Some(_) => (format!("{}LL", clean), CT::I64),
                        None => {
                            self.err(e.span, "this number is too large");
                            return None;
                        }
                    },
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
            ExprKind::Ident(name) => {
                if let Some(t) = self.lookup(name) {
                    return Some((format!("v_{}", name), t));
                }
                // a field of `this`
                if let (Some(ci), false) = (self.class, self.in_static) {
                    if self.classes[ci].fields.iter().any(|f| f.name == *name) {
                        return self.field_of("v_this".into(), ci, name, e.span);
                    }
                }
                self.unsupported(e.span, &format!("using `{}` (a static member or an unknown name)", name));
                None
            }
            ExprKind::This => match (self.class, self.in_static) {
                (Some(ci), false) => Some(("v_this".into(), CT::Ref(ci))),
                _ => {
                    self.unsupported(e.span, "`this` here");
                    None
                }
            },
            ExprKind::Member { target, name } => {
                let (c, t) = self.expr(target)?;
                match (t, name.as_str()) {
                    (CT::Str, "length") => Some((format!("ioc_strlen({})", c), CT::I32)),
                    (CT::StrArr, "length") => Some((format!("((int32_t)({}).length)", c), CT::I32)),
                    (CT::Obj(ci), _) | (CT::Ref(ci), _) => self.field_of(c, ci, name, e.span),
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
            ExprKind::New { ty, args } => self.new_expr(ty, args, e.span),
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
            ExprKind::Assign { op, target, value } => self.assign(*op, target, value),
            ExprKind::Postfix { op, expr } => {
                if !matches!(expr.kind, ExprKind::Ident(_) | ExprKind::Member { .. }) {
                    self.unsupported(expr.span, "`++`/`--` on this expression");
                    return None;
                }
                let (c, t) = self.expr(expr)?;
                if !t.is_num() {
                    self.unsupported(expr.span, "`++`/`--` on this type");
                    return None;
                }
                let o = if *op == PostOp::Inc { "++" } else { "--" };
                Some((format!("({}{})", c, o), t))
            }
            ExprKind::Borrow { expr, .. } => {
                let (c, t) = self.expr(expr)?;
                match t {
                    CT::Obj(i) | CT::Ref(i) => Some((c, CT::Ref(i))),
                    _ => {
                        self.unsupported(e.span, "`ref` / `mut ref` of a number or string");
                        None
                    }
                }
            }
            ExprKind::Base => {
                self.unsupported(e.span, "`base`");
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

    fn assign(&mut self, op: AssignOp, target: &Expr, value: &Expr) -> Option<(String, CT)> {
        if !matches!(target.kind, ExprKind::Ident(_) | ExprKind::Member { .. }) {
            self.unsupported(target.span, "assigning to this kind of place");
            return None;
        }
        let (var, tt) = self.expr(target)?;
        if var.contains("ioc_") && var.ends_with(')') && !var.starts_with("((") {
            self.unsupported(target.span, "assigning to a property with a custom getter");
            return None;
        }
        // owned object: free the old one after the new value is computed
        if let CT::Obj(i) = tt {
            let (v, vt) = self.owned_value(value)?;
            self.check_owned(tt, vt, value.span)?;
            return Some((format!("({{ ioc_c{0} *ioc_n = {1}; ioc_drop_{0}({2}); {2} = ioc_n; ioc_n; }})", i, v, var), CT::Ref(i)));
        }
        let (v, vt) = self.expr(value)?;
        let c = match op {
            AssignOp::Assign => format!("({} = {})", var, v),
            AssignOp::Add if tt == CT::Str => {
                let s = self.to_str(v, vt, value.span)?;
                format!("({} = {})", var, Self::cat(vec![var.clone(), s]))
            }
            AssignOp::Div | AssignOp::Rem if tt.is_int() => {
                let f = arith_fn(op == AssignOp::Div, tt.is_unsigned() || vt.is_unsigned());
                format!("({} = ({}){}({}, {}))", var, tt.c(), f, var, v)
            }
            AssignOp::Rem => format!("({} = fmod({}, {}))", var, var, v),
            other => format!("({} {} {})", var, other.as_str(), v),
        };
        Some((c, tt))
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
                if !(lt.is_num() || lt == CT::Bool || lt == CT::Char) {
                    self.unsupported(span, "comparing these values");
                    return None;
                }
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

    // ---- calls

    fn eval_args(&mut self, args: &[Expr]) -> Option<Vec<Arg>> {
        let mut out = Vec::new();
        let mut failed = false;
        for a in args {
            match self.expr(a) {
                Some((code, ct)) => {
                    let kind = match (&a.kind, ct) {
                        (ExprKind::Ident(n), CT::Obj(_)) if self.lookup(n).is_some() => ArgKind::Local,
                        (ExprKind::Ident(_) | ExprKind::Member { .. } | ExprKind::Index { .. } | ExprKind::This, CT::Obj(_)) => ArgKind::Place,
                        _ => ArgKind::Plain,
                    };
                    out.push(Arg { code, ct, span: a.span, kind });
                }
                None => failed = true,
            }
        }
        if failed {
            None
        } else {
            Some(out)
        }
    }

    /// Pick the best of several candidates (by parameter types) for the given arguments.
    fn pick(cands: &[(usize, Vec<CT>)], av: &[Arg]) -> Option<usize> {
        let mut best: Option<(usize, usize)> = None;
        for (id, params) in cands {
            if params.len() != av.len() {
                continue;
            }
            let mut score = 0;
            let mut ok = true;
            for (pt, a) in params.iter().zip(av.iter()) {
                let at = a.ct;
                if *pt == at {
                    score += 3;
                } else if matches!((pt, at), (CT::Ref(x), CT::Obj(y)) | (CT::Ref(x), CT::Ref(y)) if *x == y) {
                    score += 2;
                } else if pt.is_num() && at.is_num() && (pt.is_float() || !at.is_float()) {
                    score += 1;
                } else {
                    ok = false;
                }
            }
            if ok && best.map_or(true, |(s, _)| score > s) {
                best = Some((score, *id));
            }
        }
        best.map(|b| b.1)
    }

    fn call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> Option<(String, CT)> {
        let av = self.eval_args(args)?;
        match &callee.kind {
            ExprKind::Ident(name) => match self.class {
                Some(ci) => self.user_call(ci, name, av, None, span),
                None => {
                    self.unsupported(span, "this call");
                    None
                }
            },
            ExprKind::Member { target, name } => {
                if let ExprKind::Ident(q) = &target.kind {
                    if self.lookup(q).is_none() && !self.is_this_field(q) {
                        return match q.as_str() {
                            "Console" => self.console(name, av, span),
                            "Math" => self.math(name, av, span),
                            _ => match self.find_class(q) {
                                Some(ci) => self.user_call(ci, name, av, None, span),
                                None => {
                                    self.unsupported(span, &format!("calling `{}.{}`", q, name));
                                    None
                                }
                            },
                        };
                    }
                }
                let (tc, tt) = self.expr(target)?;
                match tt {
                    CT::Obj(ci) | CT::Ref(ci) => self.user_call(ci, name, av, Some(tc), span),
                    _ => self.value_method(tc, tt, name, av, span),
                }
            }
            _ => {
                self.unsupported(span, "calling this kind of expression");
                None
            }
        }
    }

    fn is_this_field(&self, name: &str) -> bool {
        match (self.class, self.in_static) {
            (Some(ci), false) => self.classes[ci].fields.iter().any(|f| f.name == name),
            _ => false,
        }
    }

    fn user_call(&mut self, ci: usize, name: &str, av: Vec<Arg>, recv: Option<String>, span: Span) -> Option<(String, CT)> {
        let cands: Vec<(usize, Vec<CT>)> = self
            .methods
            .iter()
            .enumerate()
            .filter(|(_, m)| m.class_idx == ci && m.name == name)
            .map(|(i, m)| (i, m.params.iter().map(|p| p.1).collect()))
            .collect();
        let Some(i) = Self::pick(&cands, &av) else {
            let cn = self.classes[ci].name.clone();
            self.err(span, format!("the backend found no method `{}.{}` taking these arguments", cn, name));
            return None;
        };
        let (c_name, ret, is_static, params) = {
            let m = &self.methods[i];
            (m.c_name.clone(), m.ret, m.is_static, m.params.clone())
        };
        let mut list: Vec<String> = Vec::new();
        if !is_static {
            match recv {
                Some(r) => list.push(r),
                None if !self.in_static && self.class == Some(ci) => list.push("v_this".into()),
                None => {
                    self.err(span, format!("`{}` is an instance method and needs an object", name));
                    return None;
                }
            }
        }
        for (a, (_, pt)) in av.iter().zip(params.iter()) {
            list.push(self.arg_for(a, *pt)?);
        }
        Some((format!("{}({})", c_name, list.join(", ")), ret))
    }

    fn new_expr(&mut self, ty: &TypeRef, args: &[Expr], span: Span) -> Option<(String, CT)> {
        let av = self.eval_args(args)?;
        let Some(CT::Obj(ci)) = self.type_ct(ty) else {
            self.unsupported(span, "`new` of this type");
            return None;
        };
        if self.classes[ci].ctors.is_empty() {
            if !av.is_empty() {
                self.err(span, "this class has no constructor that takes arguments");
                return None;
            }
            return Some((format!("ioc_new_{}_default()", ci), CT::Obj(ci)));
        }
        let cands: Vec<(usize, Vec<CT>)> =
            self.classes[ci].ctors.iter().enumerate().map(|(k, c)| (k, c.params.iter().map(|p| p.1).collect())).collect();
        let Some(k) = Self::pick(&cands, &av) else {
            let cn = self.classes[ci].name.clone();
            self.err(span, format!("the backend found no constructor of `{}` taking these arguments", cn));
            return None;
        };
        let (c_name, params) = {
            let c = &self.classes[ci].ctors[k];
            (c.c_name.clone(), c.params.clone())
        };
        let mut list = Vec::new();
        for (a, (_, pt)) in av.iter().zip(params.iter()) {
            list.push(self.arg_for(a, *pt)?);
        }
        Some((format!("{}({})", c_name, list.join(", ")), CT::Obj(ci)))
    }

    fn console(&mut self, name: &str, av: Vec<Arg>, span: Span) -> Option<(String, CT)> {
        let f = match name {
            "println" => "ioc_println",
            "print" => "ioc_print",
            "error" => "ioc_eprintln",
            _ => {
                self.unsupported(span, &format!("`Console.{}`", name));
                return None;
            }
        };
        if av.len() != 1 || av[0].ct != CT::Str {
            self.unsupported(span, "this use of `Console`");
            return None;
        }
        Some((format!("{}({})", f, av[0].code), CT::Void))
    }

    fn math(&mut self, name: &str, av: Vec<Arg>, span: Span) -> Option<(String, CT)> {
        let any_float = av.iter().any(|a| a.ct.is_float());
        let cs: Vec<String> = av.iter().map(|a| a.code.clone()).collect();
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

    fn value_method(&mut self, tc: String, tt: CT, name: &str, av: Vec<Arg>, span: Span) -> Option<(String, CT)> {
        let a: Vec<String> = av.iter().map(|x| x.code.clone()).collect();
        match (tt, name, a.len()) {
            (t, "toString", 0) if t != CT::StrArr && t != CT::Void && !matches!(t, CT::Obj(_) | CT::Ref(_)) => {
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
