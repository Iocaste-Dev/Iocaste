//! C backend (v0.0.5-alpha): turns a type-checked program into one C file.
//!
//! Supported so far: classes (fields, auto-properties, constructors, destructors, static and instance
//! methods with overloads, `new`, `this`), ownership (values are freed when their owner goes out of
//! scope; moving leaves NULL behind), `ref` / `mut ref` to objects; the primitive types, `string`,
//! `string[]` (command-line arguments); `let`/`var`; `if`/`while`/`for`; arithmetic; string
//! interpolation and `+`; `Console` and `Math`; a few `string` methods.
//! Enums with data (payloads must be plain values: numbers, strings, other such enums), `match`,
//! `Result<T,E>` / `Option<T>` (`T?`), `?`, `??`, `int.tryParse`.
//! Everything else reports "not supported by the C backend yet".

use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::sema::TypeTable;
use crate::ty::TyInfo;
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
    /// an enum value (user enum, or an instance of Option / Result)
    Enum(usize),
}

impl CT {
    fn c(self) -> String {
        match self {
            CT::Obj(i) | CT::Ref(i) => return format!("ioc_c{}*", i),
            CT::Enum(i) => return format!("ioc_e{}", i),
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
            CT::Obj(_) | CT::Ref(_) | CT::Enum(_) => unreachable!(),
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

impl CT {
    /// The type of a value after it was converted for storing as `self`.
    fn max_with(self, from: CT) -> CT {
        if matches!(self, CT::Enum(_)) || from == self {
            self
        } else {
            from
        }
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
    /// virtual, override, abstract, or declared in an interface: calls go through a dispatcher
    is_virtual: bool,
    is_abstract: bool,
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
    base: Option<usize>,
    ifaces: Vec<usize>,
    is_interface: bool,
    is_abstract: bool,
}

#[derive(Clone, PartialEq)]
enum EKind {
    User,
    Option(CT),
    Result(CT, CT),
}

struct VariantI {
    name: String,
    fields: Vec<(String, CT)>,
}

struct EnumI {
    name: String,
    kind: EKind,
    variants: Vec<VariantI>,
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
    types: &'a TypeTable,
    enums: Vec<EnumI>,
    tmp: usize,
    expect: Option<CT>,
    methods: Vec<Method<'a>>,
    classes: Vec<Class<'a>>,
    slots: Vec<usize>,
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

    /// Can a value of this type sit inside an enum? (plain values only: no objects yet)
    fn payload_ok(ct: CT) -> bool {
        !matches!(ct, CT::Obj(_) | CT::Ref(_) | CT::StrArr | CT::Void)
    }

    fn find_enum(&self, name: &str) -> Option<usize> {
        self.enums.iter().position(|e| e.kind == EKind::User && e.name == name)
    }

    fn option_of(&mut self, inner: CT) -> Option<usize> {
        if !Self::payload_ok(inner) {
            return None;
        }
        if let Some(i) = self.enums.iter().position(|e| e.kind == EKind::Option(inner)) {
            return Some(i);
        }
        self.enums.push(EnumI {
            name: "Option".into(),
            kind: EKind::Option(inner),
            variants: vec![
                VariantI { name: "Some".into(), fields: vec![("value".into(), inner)] },
                VariantI { name: "None".into(), fields: vec![] },
            ],
        });
        Some(self.enums.len() - 1)
    }

    fn result_of(&mut self, ok: CT, err: CT) -> Option<usize> {
        if !Self::payload_ok(ok) || !Self::payload_ok(err) {
            return None;
        }
        if let Some(i) = self.enums.iter().position(|e| e.kind == EKind::Result(ok, err)) {
            return Some(i);
        }
        self.enums.push(EnumI {
            name: "Result".into(),
            kind: EKind::Result(ok, err),
            variants: vec![
                VariantI { name: "Ok".into(), fields: vec![("value".into(), ok)] },
                VariantI { name: "Err".into(), fields: vec![("error".into(), err)] },
            ],
        });
        Some(self.enums.len() - 1)
    }

    fn prim_ct(name: &str) -> Option<CT> {
        Some(match name {
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
        })
    }

    fn type_ct(&mut self, t: &TypeRef) -> Option<CT> {
        match &t.kind {
            TypeRefKind::Named { path, args } => {
                let last = path.last()?.as_str();
                if path.len() == 1 && args.is_empty() {
                    if let Some(p) = Self::prim_ct(last) {
                        return Some(p);
                    }
                }
                if args.len() == 1 && last == "Option" {
                    let inner = self.type_ct(&args[0])?;
                    return self.option_of(inner).map(CT::Enum);
                }
                if args.len() == 2 && last == "Result" {
                    let a = self.type_ct(&args[0])?;
                    let b = self.type_ct(&args[1])?;
                    return self.result_of(a, b).map(CT::Enum);
                }
                if !args.is_empty() {
                    return None;
                }
                if let Some(e) = self.find_enum(last) {
                    return Some(CT::Enum(e));
                }
                self.find_class(last).map(CT::Obj)
            }
            TypeRefKind::Optional(inner) => {
                let i = self.type_ct(inner)?;
                self.option_of(i).map(CT::Enum)
            }
            TypeRefKind::Array(inner) => match self.type_ct(inner) {
                Some(CT::Str) => Some(CT::StrArr),
                _ => None,
            },
            TypeRefKind::Ref { inner, .. } => match self.type_ct(inner) {
                Some(CT::Obj(i)) => Some(CT::Ref(i)),
                _ => None,
            },
        }
    }

    /// The type the checker resolved for the call / name / member at `span`.
    fn table_ct(&mut self, span: Span, tag: u8) -> Option<CT> {
        let info = self.types.get(&(self.file, span.line, span.col, tag))?.clone();
        self.info_ct(&info)
    }

    fn info_ct(&mut self, info: &TyInfo) -> Option<CT> {
        match info {
            TyInfo::Prim(n) => Self::prim_ct(n),
            TyInfo::Void => Some(CT::Void),
            TyInfo::Array(i) => match self.info_ct(i)? {
                CT::Str => Some(CT::StrArr),
                _ => None,
            },
            TyInfo::Ref(i) => match self.info_ct(i)? {
                CT::Obj(c) => Some(CT::Ref(c)),
                _ => None,
            },
            TyInfo::Named(n, args) => match (n.as_str(), args.len()) {
                ("Option", 1) => {
                    let i = self.info_ct(&args[0])?;
                    self.option_of(i).map(CT::Enum)
                }
                ("Result", 2) => {
                    let a = self.info_ct(&args[0])?;
                    let b = self.info_ct(&args[1])?;
                    self.result_of(a, b).map(CT::Enum)
                }
                (name, 0) => self.find_enum(name).map(CT::Enum).or_else(|| self.find_class(name).map(CT::Obj)),
                _ => None,
            },
            TyInfo::Unknown => None,
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
pub fn generate(files: &[&SourceFile], types: &TypeTable) -> Result<String, Vec<CError>> {
    let mut g = Gen {
        types,
        enums: Vec::new(),
        tmp: 0,
        expect: None,
        methods: Vec::new(),
        classes: Vec::new(),
        slots: Vec::new(),
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

    // bodies first: generating them may create more Option / Result instances
    for i in 0..g.classes.len() {
        g.emit_class(i);
    }
    for i in 0..g.methods.len() {
        g.emit_method(i);
    }
    if !g.errors.is_empty() {
        return Err(g.errors);
    }
    let defs = std::mem::take(&mut g.out);
    g.out.push_str(RUNTIME);
    g.out.push_str("\n/* ---- program */\n");
    for i in 0..g.classes.len() {
        g.out.push_str(&format!("typedef struct ioc_c{0} ioc_c{0};\n", i));
    }
    g.emit_enums();
    g.emit_structs();
    for i in 0..g.classes.len() {
        g.out.push_str(&format!("static void ioc_drop_{0}(ioc_c{0} *v_this);\n", i));
        if g.classes[i].is_interface {
            continue;
        }
        g.out.push_str(&format!("static void ioc_dtor_{0}(ioc_c{0} *v_this);\n", i));
        for k in 0..g.classes[i].ctors.len() {
            let isig = g.init_signature(i, k);
            g.out.push_str(&format!("static {};\n", isig));
            if g.concrete(i) {
                let sig = g.ctor_signature(i, k);
                g.out.push_str(&format!("static {};\n", sig));
            }
        }
        if g.classes[i].ctors.is_empty() {
            g.out.push_str(&format!("static void ioc_init_{0}_default(ioc_c{0} *v_this);\n", i));
            if g.concrete(i) {
                g.out.push_str(&format!("static ioc_c{0} *ioc_new_{0}_default(void);\n", i));
            }
        }
    }
    for i in 0..g.slots.len() {
        let p = g.dispatcher_proto(g.slots[i]);
        g.out.push_str(&format!("{};\n", p));
    }
    for i in 0..g.methods.len() {
        if g.methods[i].is_abstract {
            continue;
        }
        let sig = g.signature(i);
        g.out.push_str(&format!("static {};\n", sig));
    }
    g.out.push('\n');
    g.out.push_str(&defs);
    let disp = g.emit_dispatchers();
    g.out.push_str(&disp);
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
            diag: Diagnostic::new(format!("{} is not supported by the C backend yet (v0.0.9)", what), span)
                .with_hint("`ioc check` still validates it; code generation for it arrives in the next versions"),
        });
    }

    // ------------------------------------------------------- collecting

    fn register_items(&mut self, items: &'a [Item]) {
        for it in items {
            match it {
                Item::Namespace(n) => self.register_items(&n.items),
                Item::Type(t) => self.register_type(t),
                Item::Enum(e) => self.register_enum(e),
                _ => {}
            }
        }
    }

    fn register_enum(&mut self, e: &'a EnumDecl) {
        if e.generics.is_empty() {
            self.enums.push(EnumI { name: e.name.clone(), kind: EKind::User, variants: Vec::new() });
        }
    }

    fn collect_enum(&mut self, e: &'a EnumDecl) {
        if !e.generics.is_empty() {
            self.unsupported(e.span, "a generic enum");
            return;
        }
        let Some(ei) = self.find_enum(&e.name) else { return };
        let mut variants = Vec::new();
        for v in &e.variants {
            if v.value.is_some() {
                self.unsupported(v.span, "an enum variant with an explicit value");
            }
            let mut fields = Vec::new();
            for f in &v.fields {
                match self.type_ct(&f.ty) {
                    Some(ct) if Self::payload_ok(ct) => fields.push((f.name.clone(), ct)),
                    _ => self.unsupported(f.ty.span, "this enum payload type (objects inside enums come later)"),
                }
            }
            variants.push(VariantI { name: v.name.clone(), fields });
        }
        self.enums[ei].variants = variants;
    }

    fn register_type(&mut self, t: &'a TypeDecl) {
        if matches!(t.kind, TypeKind::Class | TypeKind::Interface) && t.generics.is_empty() {
            self.classes.push(Class {
                name: t.name.clone(),
                file: self.file,
                span: t.span,
                fields: Vec::new(),
                ctors: Vec::new(),
                dtor: None,
                base: None,
                ifaces: Vec::new(),
                is_interface: t.kind == TypeKind::Interface,
                is_abstract: t.modifiers.contains(Modifier::Abstract),
            });
        }
    }

    fn collect_items(&mut self, items: &'a [Item], next_class: &mut usize) {
        for it in items {
            match it {
                Item::Namespace(n) => self.collect_items(&n.items, next_class),
                Item::Import(_) => {}
                Item::Enum(e) => self.collect_enum(e),
                Item::Type(t) => self.collect_type(t, next_class),
            }
        }
    }

    fn collect_type(&mut self, t: &'a TypeDecl, next_class: &mut usize) {
        if !matches!(t.kind, TypeKind::Class | TypeKind::Interface) {
            self.unsupported(t.span, if t.kind == TypeKind::Struct { "`struct`" } else { "`record`" });
            return;
        }
        if !t.generics.is_empty() {
            self.unsupported(t.span, "a generic class");
            return;
        }
        let ci = *next_class;
        *next_class += 1;
        for b in &t.bases {
            match self.type_ct(b) {
                Some(CT::Obj(bi)) => {
                    if self.classes[bi].is_interface {
                        self.classes[ci].ifaces.push(bi);
                    } else if self.classes[ci].base.is_none() && !self.classes[ci].is_interface {
                        self.classes[ci].base = Some(bi);
                    }
                }
                _ => self.unsupported(b.span, "this base type"),
            }
        }
        let is_iface = t.kind == TypeKind::Interface;
        for m in &t.members {
            if is_iface && !matches!(m, Member::Method(_)) {
                self.unsupported(member_span(m), "this kind of interface member");
                continue;
            }
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
                Member::Enum(e) => self.unsupported(e.span, "an enum nested in a class"),
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
            is_virtual: false,
            is_abstract: false,
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
            is_virtual: self.classes[ci].is_interface
                || md.modifiers.contains(Modifier::Virtual)
                || md.modifiers.contains(Modifier::Override)
                || md.modifiers.contains(Modifier::Abstract),
            is_abstract: self.classes[ci].is_interface || md.modifiers.contains(Modifier::Abstract),
            body: &md.body,
            span: md.span,
            file: self.file,
        });
    }

    // ------------------------------------------------------- output

    /// Struct definitions of all enums, payload enums first.
    fn emit_enums(&mut self) {
        let n = self.enums.len();
        let mut done = vec![false; n];
        fn visit(g: &mut Gen, i: usize, done: &mut Vec<bool>) {
            if done[i] {
                return;
            }
            done[i] = true;
            let deps: Vec<usize> = g.enums[i]
                .variants
                .iter()
                .flat_map(|v| v.fields.iter())
                .filter_map(|(_, ct)| if let CT::Enum(j) = ct { Some(*j) } else { None })
                .collect();
            for d in deps {
                visit(g, d, done);
            }
            let mut s = format!("typedef struct {{\n    int tag;\n    union {{\n        char pad_;\n");
            for (vi, v) in g.enums[i].variants.iter().enumerate() {
                if v.fields.is_empty() {
                    continue;
                }
                s.push_str("        struct { ");
                for (fname, ct) in &v.fields {
                    s.push_str(&format!("{} f_{}; ", ct.c(), fname));
                }
                s.push_str(&format!("}} v{};\n", vi));
            }
            s.push_str(&format!("    }} u;\n}} ioc_e{};\n", i));
            g.out.push_str(&s);
        }
        for i in 0..n {
            visit(self, i, &mut done);
        }
    }

    fn emit_structs(&mut self) {
        let n = self.classes.len();
        let mut done = vec![false; n];
        fn visit(g: &mut Gen, i: usize, done: &mut Vec<bool>) {
            if done[i] {
                return;
            }
            done[i] = true;
            if let Some(b) = g.classes[i].base {
                visit(g, b, done);
            }
            let mut s = format!("struct ioc_c{} {{\n", i);
            let c = &g.classes[i];
            match c.base {
                Some(b) => s.push_str(&format!("    ioc_c{} b_;\n", b)),
                None => s.push_str("    int ioc_cls;\n"),
            }
            for f in &c.fields {
                if f.getter.is_none() {
                    s.push_str(&format!("    {} f_{};\n", f.ct.c(), f.name));
                }
            }
            s.push_str("};\n");
            g.out.push_str(&s);
        }
        for i in 0..n {
            visit(self, i, &mut done);
        }
    }

    // ---- class hierarchy helpers

    /// The class, its base classes (or, for an interface, the interfaces it extends).
    fn chain(&self, ci: usize) -> Vec<usize> {
        let mut out = vec![ci];
        if self.classes[ci].is_interface {
            let mut i = 0;
            while i < out.len() {
                for &b in &self.classes[out[i]].ifaces.clone() {
                    if !out.contains(&b) {
                        out.push(b);
                    }
                }
                i += 1;
            }
        } else {
            let mut c = ci;
            while let Some(b) = self.classes[c].base {
                out.push(b);
                c = b;
            }
        }
        out
    }

    /// Is `a` the same as `b`, derived from it, or an implementation of it?
    fn is_sub(&self, a: usize, b: usize) -> bool {
        if a == b {
            return true;
        }
        let mut seen = vec![a];
        let mut i = 0;
        while i < seen.len() {
            let c = seen[i];
            i += 1;
            let mut next: Vec<usize> = self.classes[c].ifaces.clone();
            if let Some(bs) = self.classes[c].base {
                next.push(bs);
            }
            for n in next {
                if n == b {
                    return true;
                }
                if !seen.contains(&n) {
                    seen.push(n);
                }
            }
        }
        false
    }

    fn concrete(&self, ci: usize) -> bool {
        !self.classes[ci].is_interface && !self.classes[ci].is_abstract
    }

    /// Where field `name` lives: (class that declares it, index in that class).
    fn find_field(&self, ci: usize, name: &str) -> Option<(usize, usize)> {
        for c in self.chain(ci) {
            if let Some(i) = self.classes[c].fields.iter().position(|f| f.name == name) {
                return Some((c, i));
            }
        }
        None
    }

    /// `obj` (a pointer to class `from`) seen as a pointer to its ancestor `to`.
    fn upcast(&self, obj: &str, from: usize, to: usize) -> String {
        if from == to {
            obj.to_string()
        } else {
            format!("((ioc_c{}*)({}))", to, obj)
        }
    }

    /// Slot dispatcher for virtual method `m`: calls the implementation matching the object's real class.
    fn slot_call(&mut self, m: usize, recv: String, args: Vec<String>) -> String {
        if !self.slots.contains(&m) {
            self.slots.push(m);
        }
        let mut all = vec![recv];
        all.extend(args);
        format!("ioc_vcall_{}({})", m, all.join(", "))
    }

    /// The method that class `c` runs for the virtual method `m0`.
    fn impl_for(&self, c: usize, m0: usize) -> Option<usize> {
        let (name, params) = (&self.methods[m0].name, &self.methods[m0].params);
        for x in self.chain(c) {
            for (i, m) in self.methods.iter().enumerate() {
                if m.class_idx == x && m.name == *name && m.params == *params && !m.is_static && !m.is_abstract {
                    return Some(i);
                }
            }
        }
        None
    }

    fn dispatcher_proto(&self, m: usize) -> String {
        let me = &self.methods[m];
        let mut params = vec![format!("ioc_c{}* o", me.class_idx)];
        params.extend(me.params.iter().enumerate().map(|(i, (_, t))| format!("{} a{}", t.c(), i)));
        format!("static {} ioc_vcall_{}({})", me.ret.c(), m, params.join(", "))
    }

    fn emit_dispatchers(&mut self) -> String {
        let mut out = String::new();
        let mut i = 0;
        while i < self.slots.len() {
            let m = self.slots[i];
            i += 1;
            let proto = self.dispatcher_proto(m);
            let me_class = self.methods[m].class_idx;
            let ret = self.methods[m].ret;
            let n_params = self.methods[m].params.len();
            out.push_str(&format!("{} {{\n    switch (IOC_CLS(o)) {{\n", proto));
            for c in 0..self.classes.len() {
                if !self.concrete(c) || !self.is_sub(c, me_class) {
                    continue;
                }
                let Some(k) = self.impl_for(c, m) else { continue };
                let impl_class = self.methods[k].class_idx;
                let mut args = vec![format!("(ioc_c{}*)o", impl_class)];
                args.extend((0..n_params).map(|j| format!("a{}", j)));
                let call = format!("{}({})", self.methods[k].c_name, args.join(", "));
                if ret == CT::Void {
                    out.push_str(&format!("        case {}: {}; return;\n", c, call));
                } else {
                    out.push_str(&format!("        case {}: return {};\n", c, call));
                }
            }
            out.push_str("        default: ioc_panic(\"bad virtual call\");\n    }\n");
            if ret != CT::Void {
                out.push_str(&format!("    return ({}){{0}};\n", ret.c()));
            }
            out.push_str("}\n\n");
        }
        out
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

    fn ctor_params(&self, ci: usize, k: usize) -> String {
        let c = &self.classes[ci].ctors[k];
        let params: Vec<String> = c.params.iter().map(|(n, t)| format!("{} v_{}", t.c(), n)).collect();
        if params.is_empty() { "void".to_string() } else { params.join(", ") }
    }

    /// `static void ioc_init_C_k(ioc_cC* v_this, ...)`: runs base init, field initializers and the body.
    fn init_signature(&self, ci: usize, k: usize) -> String {
        let c = &self.classes[ci].ctors[k];
        let mut params = vec![format!("ioc_c{}* v_this", ci)];
        params.extend(c.params.iter().map(|(n, t)| format!("{} v_{}", t.c(), n)));
        format!("void {}({})", c.c_name.replace("ioc_new_", "ioc_init_"), params.join(", "))
    }

    /// `ioc_cC* ioc_new_C_k(...)`: allocates and initializes.
    fn ctor_signature(&self, ci: usize, k: usize) -> String {
        format!("ioc_c{}* {}({})", ci, self.classes[ci].ctors[k].c_name, self.ctor_params(ci, k))
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
                    self.line("return;");
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
        self.in_static = false;
        self.ret = CT::Void;
        self.scopes = vec![Scope::default()];

        // ---- destructor chain (runs the destructor body, frees owned fields, then the base part)
        if !self.classes[ci].is_interface {
            self.ctx = Ctx::Dtor;
            self.line(&format!("static void ioc_dtor_{0}(ioc_c{0} *v_this) {{", ci));
            self.ind += 1;
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
            if let Some(b) = self.classes[ci].base {
                self.line(&format!("ioc_dtor_{0}((ioc_c{0} *)v_this);", b));
            }
            self.ind -= 1;
            self.line("}");
            self.out.push('\n');
        }
        // ---- drop: finds the real class, runs its destructor chain, frees the memory once
        self.line(&format!("static void ioc_drop_{0}(ioc_c{0} *v_this) {{", ci));
        self.ind += 1;
        self.line("if (!v_this) return;");
        self.line("switch (IOC_CLS(v_this)) {");
        for c in 0..self.classes.len() {
            if self.concrete(c) && self.is_sub(c, ci) {
                self.line(&format!("    case {0}: ioc_dtor_{0}((ioc_c{0} *)v_this); break;", c));
            }
        }
        self.line("    default: break;");
        self.line("}");
        self.line("free(v_this);");
        self.ind -= 1;
        self.line("}");
        self.out.push('\n');
        if self.classes[ci].is_interface {
            self.class = None;
            return;
        }

        // ---- constructors
        self.ctx = Ctx::Ctor;
        if self.classes[ci].ctors.is_empty() {
            self.scopes = vec![Scope::default()];
            self.line(&format!("static void ioc_init_{0}_default(ioc_c{0} *v_this) {{", ci));
            self.ind += 1;
            self.emit_base_init(ci, None);
            self.emit_field_inits(ci);
            self.ind -= 1;
            self.line("}");
            self.out.push('\n');
            if self.concrete(ci) {
                self.line(&format!("static ioc_c{0} *ioc_new_{0}_default(void) {{", ci));
                self.ind += 1;
                self.line(&format!("ioc_c{0} *v_this = (ioc_c{0} *)ioc_zalloc(sizeof(ioc_c{0}));", ci));
                self.line(&format!("IOC_CLS(v_this) = {};", ci));
                self.line(&format!("ioc_init_{}_default(v_this);", ci));
                self.line("return v_this;");
                self.ind -= 1;
                self.line("}");
                self.out.push('\n');
            }
        }
        for k in 0..self.classes[ci].ctors.len() {
            let sig = self.init_signature(ci, k);
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
            // `base(args)` must come first
            let (base_call, rest): (Option<&'a [Expr]>, &'a [Stmt]) = match decl.body.stmts.first().map(|s| &s.kind) {
                Some(StmtKind::Expr(Expr { kind: ExprKind::Call { callee, args }, .. })) if matches!(callee.kind, ExprKind::Base) => {
                    (Some(args.as_slice()), &decl.body.stmts[1..])
                }
                _ => (None, &decl.body.stmts[..]),
            };
            self.emit_base_init(ci, base_call);
            self.emit_field_inits(ci);
            self.push_scope();
            for s in rest {
                self.stmt(s);
            }
            self.pop_scope();
            self.drop_lines(0);
            self.ind -= 1;
            self.line("}");
            self.out.push('\n');
            if self.concrete(ci) {
                let new_sig = self.ctor_signature(ci, k);
                let names: Vec<String> = params.iter().map(|(n, _)| format!("v_{}", n)).collect();
                let init_name = self.classes[ci].ctors[k].c_name.replace("ioc_new_", "ioc_init_");
                self.line(&format!("static {} {{", new_sig));
                self.ind += 1;
                self.line(&format!("ioc_c{0} *v_this = (ioc_c{0} *)ioc_zalloc(sizeof(ioc_c{0}));", ci));
                self.line(&format!("IOC_CLS(v_this) = {};", ci));
                let mut a = vec!["v_this".to_string()];
                a.extend(names);
                self.line(&format!("{}({});", init_name, a.join(", ")));
                self.line("return v_this;");
                self.ind -= 1;
                self.line("}");
                self.out.push('\n');
            }
        }
        self.ctx = Ctx::Method;
        self.class = None;
    }

    /// Run the base class initializer: the explicit `base(args)`, or its constructor without arguments.
    fn emit_base_init(&mut self, ci: usize, explicit: Option<&'a [Expr]>) {
        let Some(b) = self.classes[ci].base else { return };
        let args = explicit.unwrap_or(&[]);
        let Some(av) = self.eval_args(args) else { return };
        if self.classes[b].ctors.is_empty() {
            if !av.is_empty() {
                self.err(self.classes[ci].span, "the base class has no constructor that takes arguments");
                return;
            }
            self.line(&format!("ioc_init_{0}_default((ioc_c{0} *)v_this);", b));
            return;
        }
        let cands: Vec<(usize, Vec<CT>)> =
            self.classes[b].ctors.iter().enumerate().map(|(k, c)| (k, c.params.iter().map(|p| p.1).collect())).collect();
        let Some(k) = self.pick(&cands, &av) else {
            let span = self.classes[ci].span;
            self.err(span, "the backend found no base constructor taking these arguments");
            return;
        };
        let params = self.classes[b].ctors[k].params.clone();
        let init = self.classes[b].ctors[k].c_name.replace("ioc_new_", "ioc_init_");
        let mut list = vec![format!("(ioc_c{}*)v_this", b)];
        for (a, (_, pt)) in av.iter().zip(params.iter()) {
            match self.arg_for(a, *pt) {
                Some(c) => list.push(c),
                None => return,
            }
        }
        self.line(&format!("{}({});", init, list.join(", ")));
    }

    fn emit_field_inits(&mut self, ci: usize) {
        let inits: Vec<(String, &'a Expr)> =
            self.classes[ci].fields.iter().filter_map(|f| f.init.map(|e| (f.name.clone(), e))).collect();
        for (n, e) in inits {
            let fct = self.classes[ci].fields.iter().find(|f| f.name == n).map(|f| f.ct);
            if let Some((c, _)) = self.owned_value_as(e, fct) {
                self.line(&format!("v_this->f_{} = {};", n, c));
            }
        }
    }

    fn emit_method(&mut self, i: usize) {
        if self.methods[i].is_abstract {
            return;
        }
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
                } else if let Some(v) = self.owned_value_as(e, Some(ret)) {
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
                    Some(e) => match self.owned_value_as(e, declared) {
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
                if let ExprKind::Match { scrutinee, arms } = &e.kind {
                    self.match_gen(scrutinee, arms, false, e.span);
                    return;
                }
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
                let c = self.no_hoist(cond.span, "a `match` or `?` in a loop condition", |g| g.expr(cond)).map(|x| x.0).unwrap_or_default();
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
                    Some(c) => self.no_hoist(c.span, "a `match` or `?` in a loop condition", |g| g.expr(c)).map(|x| x.0).unwrap_or_default(),
                    None => String::new(),
                };
                let u = match update {
                    Some(u) => self.no_hoist(u.span, "a `match` or `?` in a loop update", |g| g.expr(u)).map(|x| x.0).unwrap_or_default(),
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
                    if let Some(val) = self.owned_value_as(e, Some(self.ret)) {
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
                ArgKind::Local => Some(self.coerce(format!("IOC_TAKE({})", a.code), a.ct, param)),
                ArgKind::Place => {
                    self.unsupported(a.span, "moving an object out of a field (borrow it with `ref` instead)");
                    None
                }
                ArgKind::Plain => Some(self.coerce(a.code.clone(), a.ct, param)),
            };
        }
        Some(self.coerce(a.code.clone(), a.ct, param))
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
        let Some((owner, fi)) = self.find_field(ci, name) else {
            self.unsupported(span, &format!("the member `{}`", name));
            return None;
        };
        let (ct, getter) = {
            let f = &self.classes[owner].fields[fi];
            (f.ct, f.getter)
        };
        let o = self.upcast(&obj, ci, owner);
        match getter {
            Some(k) => Some((format!("{}({})", self.methods[k].c_name, o), ct)),
            None => Some((format!("(({})->f_{})", o, name), ct)),
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
                if name == "None" {
                    return self.builtin_variant("None", Vec::new(), e.span, 2);
                }
                // a field of `this`
                if let (Some(ci), false) = (self.class, self.in_static) {
                    if self.find_field(ci, name).is_some() {
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
                if let ExprKind::Ident(q) = &target.kind {
                    if self.lookup(q).is_none() && !self.is_this_field(q) && self.find_enum(q).is_some() {
                        return self.enum_call(q, name, Vec::new(), e.span);
                    }
                }
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
            ExprKind::Binary { op: BinOp::Coalesce, lhs, rhs } => self.coalesce(lhs, rhs, e.span),
            ExprKind::Binary { op: op @ (BinOp::And | BinOp::Or), lhs, rhs } => {
                let (lc, _) = self.expr(lhs)?;
                let (rc, _) = self.no_hoist(rhs.span, "a `match` or `?` on the right of `&&` / `||`", |g| g.expr(rhs))?;
                Some((format!("({} {} {})", lc, if *op == BinOp::And { "&&" } else { "||" }, rc), CT::Bool))
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
            ExprKind::Try(inner) => self.try_expr(inner, e.span),
            ExprKind::Await(_) => {
                self.unsupported(e.span, "`await`");
                None
            }
            ExprKind::Match { scrutinee, arms } => self.match_gen(scrutinee, arms, true, e.span),
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
        let saved = self.expect;
        self.expect = Some(tt);
        let r = self.expr(value);
        self.expect = saved;
        let (v, vt) = r?;
        let v = if op == AssignOp::Assign { self.coerce(v, vt, tt) } else { v };
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
    fn pick(&self, cands: &[(usize, Vec<CT>)], av: &[Arg]) -> Option<usize> {
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
                } else if matches!((pt, at), (CT::Ref(x) | CT::Obj(x), CT::Obj(y) | CT::Ref(y)) if self.is_sub(y, *x)) {
                    score += 1;
                } else if pt.is_num() && at.is_num() && (pt.is_float() || !at.is_float()) {
                    score += 1;
                } else if self.can_store(*pt, at) {
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
            ExprKind::Ident(name) if matches!(name.as_str(), "Ok" | "Err" | "Some") && self.lookup(name).is_none() => {
                self.builtin_variant(name, av, span, 1)
            }
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
                        if self.find_enum(q).is_some() {
                            return self.enum_call(q, name, av, span);
                        }
                        if name == "tryParse" && Self::prim_ct(q).is_some() {
                            return self.try_parse(q, av, span);
                        }
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
                if let ExprKind::Base = target.kind {
                    return self.base_method_call(name, av, span);
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

    /// `base.name(args)`: the base class's own implementation, called directly (not through dispatch).
    fn base_method_call(&mut self, name: &str, av: Vec<Arg>, span: Span) -> Option<(String, CT)> {
        let (Some(ci), false) = (self.class, self.in_static) else {
            self.err(span, "`base` can only be used inside an instance method");
            return None;
        };
        let Some(b) = self.classes[ci].base else {
            self.err(span, "this class has no base class");
            return None;
        };
        let mut cands: Vec<(usize, Vec<CT>)> = Vec::new();
        for c in self.chain(b) {
            for (i, m) in self.methods.iter().enumerate() {
                if m.class_idx == c && m.name == name && !m.is_abstract {
                    let sig: Vec<CT> = m.params.iter().map(|p| p.1).collect();
                    if !cands.iter().any(|(_, s)| *s == sig) {
                        cands.push((i, sig));
                    }
                }
            }
        }
        let Some(i) = self.pick(&cands, &av) else {
            self.err(span, format!("the base class has no method `{}` taking these arguments", name));
            return None;
        };
        let (c_name, ret, params, mclass) = {
            let m = &self.methods[i];
            (m.c_name.clone(), m.ret, m.params.clone(), m.class_idx)
        };
        let mut list = vec![format!("((ioc_c{}*)v_this)", mclass)];
        for (a, (_, pt)) in av.iter().zip(params.iter()) {
            list.push(self.arg_for(a, *pt)?);
        }
        Some((format!("{}({})", c_name, list.join(", ")), ret))
    }

    fn is_this_field(&self, name: &str) -> bool {
        match (self.class, self.in_static) {
            (Some(ci), false) => self.find_field(ci, name).is_some(),
            _ => false,
        }
    }

    fn user_call(&mut self, ci: usize, name: &str, av: Vec<Arg>, recv: Option<String>, span: Span) -> Option<(String, CT)> {
        // methods visible on `ci`: its own first, then inherited ones; an override hides the method it replaces
        let mut cands: Vec<(usize, Vec<CT>)> = Vec::new();
        for c in self.chain(ci) {
            for (i, m) in self.methods.iter().enumerate() {
                if m.class_idx == c && m.name == name {
                    let sig: Vec<CT> = m.params.iter().map(|p| p.1).collect();
                    if !cands.iter().any(|(_, s)| *s == sig) {
                        cands.push((i, sig));
                    }
                }
            }
        }
        let Some(i) = self.pick(&cands, &av) else {
            let cn = self.classes[ci].name.clone();
            self.err(span, format!("the backend found no method `{}.{}` taking these arguments", cn, name));
            return None;
        };
        let (c_name, ret, is_static, params, mclass, virt) = {
            let m = &self.methods[i];
            (m.c_name.clone(), m.ret, m.is_static, m.params.clone(), m.class_idx, m.is_virtual)
        };
        let mut list: Vec<String> = Vec::new();
        let mut root = i;
        let mut recv_code = String::new();
        let mut recv_class = ci;
        if !is_static {
            match recv {
                Some(r) => recv_code = r,
                None if !self.in_static && self.class.is_some() => {
                    recv_code = "v_this".into();
                    recv_class = self.class.unwrap();
                }
                None => {
                    self.err(span, format!("`{}` is an instance method and needs an object", name));
                    return None;
                }
            }
            if virt {
                // the base-most declaration owns the dispatcher
                let (n, p) = (self.methods[i].name.clone(), self.methods[i].params.clone());
                for c in self.chain(recv_class).into_iter().rev() {
                    if let Some(k) = self.methods.iter().position(|m| m.class_idx == c && m.name == n && m.params == p && m.is_virtual) {
                        root = k;
                        break;
                    }
                }
            }
            let target = if virt { self.methods[root].class_idx } else { mclass };
            list.push(self.upcast(&recv_code, recv_class, target));
        }
        let mut args = Vec::new();
        for (a, (_, pt)) in av.iter().zip(params.iter()) {
            args.push(self.arg_for(a, *pt)?);
        }
        if !is_static && virt {
            let call = self.slot_call(root, list.remove(0), args);
            return Some((call, ret));
        }
        list.extend(args);
        Some((format!("{}({})", c_name, list.join(", ")), ret))
    }

    fn new_expr(&mut self, ty: &TypeRef, args: &[Expr], span: Span) -> Option<(String, CT)> {
        let av = self.eval_args(args)?;
        let Some(CT::Obj(ci)) = self.type_ct(ty) else {
            self.unsupported(span, "`new` of this type");
            return None;
        };
        if !self.concrete(ci) {
            self.err(span, "cannot create an object of an abstract class or an interface");
            return None;
        }
        if self.classes[ci].ctors.is_empty() {
            if !av.is_empty() {
                self.err(span, "this class has no constructor that takes arguments");
                return None;
            }
            return Some((format!("ioc_new_{}_default()", ci), CT::Obj(ci)));
        }
        let cands: Vec<(usize, Vec<CT>)> =
            self.classes[ci].ctors.iter().enumerate().map(|(k, c)| (k, c.params.iter().map(|p| p.1).collect())).collect();
        let Some(k) = self.pick(&cands, &av) else {
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
        if name == "readLine" && av.is_empty() {
            let ei = self.option_of(CT::Str)?;
            let oc = CT::Enum(ei).c();
            return Some((
                format!("({{ ioc_string ioc_l = ioc_readline(); ioc_l ? ({oc}){{ .tag = 0, .u.v0 = {{ .f_value = ioc_l }} }} : ({oc}){{ .tag = 1 }}; }})", oc = oc),
                CT::Enum(ei),
            ));
        }
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


// ---------------------------------------------------------------- enums, match, Result / Option

impl<'a> Gen<'a> {
    fn fresh(&mut self, prefix: &str) -> String {
        self.tmp += 1;
        format!("{}{}", prefix, self.tmp)
    }

    fn variant_index(&self, ei: usize, name: &str) -> Option<usize> {
        self.enums[ei].variants.iter().position(|v| v.name == name)
    }

    /// Convert `code` of type `from` so it can be stored where `to` is expected:
    /// a plain `T` becomes `Some(T)` for a `T?`.
    fn coerce(&self, code: String, from: CT, to: CT) -> String {
        if from == to {
            return code;
        }
        if let (CT::Obj(x) | CT::Ref(x), CT::Obj(y) | CT::Ref(y)) = (to, from) {
            return if x == y { code } else { format!("((ioc_c{}*)({}))", x, code) };
        }
        if let CT::Enum(ei) = to {
            if let EKind::Option(inner) = self.enums[ei].kind {
                if from == inner || (from.is_num() && inner.is_num()) {
                    return format!("(({}){{ .tag = 0, .u.v0 = {{ .f_value = ({}){} }} }})", to.c(), inner.c(), code);
                }
            }
        }
        code
    }

    fn can_store(&self, param: CT, arg: CT) -> bool {
        if let CT::Enum(ei) = param {
            if let EKind::Option(inner) = self.enums[ei].kind {
                return arg == inner || (arg.is_num() && inner.is_num());
            }
        }
        false
    }

    /// `Ok(x)`, `Err(x)`, `Some(x)`, `None`, `Enum.Variant`, `Enum.Variant(args)`.
    fn make_variant(&mut self, ei: usize, vi: usize, args: Vec<Arg>, span: Span) -> Option<(String, CT)> {
        let fields = self.enums[ei].variants[vi].fields.clone();
        if fields.len() != args.len() {
            self.err(span, "wrong number of values for this variant");
            return None;
        }
        let ct = CT::Enum(ei);
        if fields.is_empty() {
            return Some((format!("(({}){{ .tag = {} }})", ct.c(), vi), ct));
        }
        let mut parts = Vec::new();
        for (a, (fname, fct)) in args.iter().zip(fields.iter()) {
            let code = self.coerce(a.code.clone(), a.ct, *fct);
            parts.push(format!(".f_{} = {}", fname, code));
        }
        Some((format!("(({}){{ .tag = {}, .u.v{} = {{ {} }} }})", ct.c(), vi, vi, parts.join(", ")), ct))
    }

    /// `Ok(..)` / `Err(..)` / `Some(..)` / `None`: the checker resolved the full type.
    fn builtin_variant(&mut self, name: &str, args: Vec<Arg>, span: Span, tag: u8) -> Option<(String, CT)> {
        let Some(CT::Enum(ei)) = self.table_ct(span, tag) else {
            self.err(span, format!("the backend cannot tell which `{}` type this is; add a type annotation", name));
            return None;
        };
        let Some(vi) = self.variant_index(ei, name) else {
            self.err(span, "unexpected variant");
            return None;
        };
        self.make_variant(ei, vi, args, span)
    }

    fn enum_call(&mut self, q: &str, name: &str, args: Vec<Arg>, span: Span) -> Option<(String, CT)> {
        let ei = self.find_enum(q)?;
        let Some(vi) = self.variant_index(ei, name) else {
            self.err(span, format!("`{}` has no variant `{}`", q, name));
            return None;
        };
        self.make_variant(ei, vi, args, span)
    }

    /// `int.tryParse(text)` and friends: returns an `Option`.
    fn try_parse(&mut self, ty: &str, av: Vec<Arg>, span: Span) -> Option<(String, CT)> {
        let target = Self::prim_ct(ty)?;
        if av.len() != 1 || av[0].ct != CT::Str {
            self.err(span, "tryParse takes one string");
            return None;
        }
        let Some(ei) = self.option_of(target) else {
            self.unsupported(span, &format!("`{}.tryParse`", ty));
            return None;
        };
        let oc = CT::Enum(ei).c();
        let tc = target.c();
        let (fun, wide) = if target.is_float() {
            ("ioc_try_f64", "double")
        } else if target.is_unsigned() {
            ("ioc_try_u64", "uint64_t")
        } else if target.is_int() {
            ("ioc_try_i64", "int64_t")
        } else {
            self.unsupported(span, &format!("`{}.tryParse`", ty));
            return None;
        };
        let (lo, hi) = if target.is_float() { ("0".to_string(), "0".to_string()) } else { (format!("({})0", wide), "0".into()) };
        let _ = (lo, hi);
        let range = match target {
            CT::I8 => ("-128", "127"),
            CT::I16 => ("-32768", "32767"),
            CT::I32 => ("-2147483648LL", "2147483647LL"),
            CT::I64 => ("(-9223372036854775807LL-1)", "9223372036854775807LL"),
            CT::U8 => ("0", "255"),
            CT::U16 => ("0", "65535"),
            CT::U32 => ("0", "4294967295ULL"),
            CT::U64 => ("0", "18446744073709551615ULL"),
            _ => ("0", "0"),
        };
        let code = if target.is_float() {
            format!("({{ {w} ioc_v; {oc} ioc_o; if ({f}({s}, &ioc_v)) {{ ioc_o = ({oc}){{ .tag = 0, .u.v0 = {{ .f_value = ({t})ioc_v }} }}; }} else {{ ioc_o = ({oc}){{ .tag = 1 }}; }} ioc_o; }})", w = wide, oc = oc, f = fun, s = av[0].code, t = tc)
        } else {
            format!("({{ {w} ioc_v; {oc} ioc_o; if ({f}({s}, &ioc_v, {lo}, {hi})) {{ ioc_o = ({oc}){{ .tag = 0, .u.v0 = {{ .f_value = ({t})ioc_v }} }}; }} else {{ ioc_o = ({oc}){{ .tag = 1 }}; }} ioc_o; }})", w = wide, oc = oc, f = fun, s = av[0].code, t = tc, lo = range.0, hi = range.1)
        };
        Some((code, CT::Enum(ei)))
    }

    /// Run `f`; refuse if it had to emit statements first (it would run once instead of every time).
    fn no_hoist<T>(&mut self, span: Span, what: &str, f: impl FnOnce(&mut Self) -> Option<T>) -> Option<T> {
        let before = self.out.len();
        let r = f(self);
        if self.out.len() != before {
            self.unsupported(span, what);
            return None;
        }
        r
    }

    /// `x?` : unwrap a Result / Option, or return early with the failure.
    fn try_expr(&mut self, inner: &Expr, span: Span) -> Option<(String, CT)> {
        let (c, t) = self.expr(inner)?;
        let CT::Enum(ei) = t else {
            self.unsupported(span, "`?` on this value");
            return None;
        };
        let tmp = self.fresh("ioc_t");
        self.line(&format!("{} {} = {};", t.c(), tmp, c));
        let ret = self.ret;
        match (self.enums[ei].kind.clone(), ret) {
            (EKind::Result(ok, _), CT::Enum(ri)) if matches!(self.enums[ri].kind, EKind::Result(..)) => {
                self.line(&format!("if ({}.tag == 1) {{", tmp));
                self.ind += 1;
                let r = format!("(({}){{ .tag = 1, .u.v1 = {{ .f_error = {}.u.v1.f_error }} }})", ret.c(), tmp);
                self.emit_return(Some((r, ret)));
                self.ind -= 1;
                self.line("}");
                Some((format!("{}.u.v0.f_value", tmp), ok))
            }
            (EKind::Option(inner_ct), CT::Enum(ri)) if matches!(self.enums[ri].kind, EKind::Option(_)) => {
                self.line(&format!("if ({}.tag == 1) {{", tmp));
                self.ind += 1;
                let r = format!("(({}){{ .tag = 1 }})", ret.c());
                self.emit_return(Some((r, ret)));
                self.ind -= 1;
                self.line("}");
                Some((format!("{}.u.v0.f_value", tmp), inner_ct))
            }
            _ => {
                self.unsupported(span, "`?` in a method that does not return a matching Result / Option");
                None
            }
        }
    }

    /// `a ?? b` where `a` is an Option.
    fn coalesce(&mut self, l: &Expr, r: &Expr, span: Span) -> Option<(String, CT)> {
        let (lc, lt) = self.expr(l)?;
        let CT::Enum(ei) = lt else {
            self.unsupported(span, "`??` on this value");
            return None;
        };
        let EKind::Option(inner) = self.enums[ei].kind.clone() else {
            self.unsupported(span, "`??` on a Result");
            return None;
        };
        let tmp = self.fresh("ioc_t");
        self.line(&format!("{} {} = {};", lt.c(), tmp, lc));
        if let ExprKind::Return(v) = &r.kind {
            self.line(&format!("if ({}.tag == 1) {{", tmp));
            self.ind += 1;
            match v {
                Some(e) => {
                    let rt = self.ret;
                    let val = self.owned_value_as(e, Some(rt))?;
                    self.emit_return(Some(val));
                }
                None => self.emit_return(None),
            }
            self.ind -= 1;
            self.line("}");
            return Some((format!("{}.u.v0.f_value", tmp), inner));
        }
        let (rc, rt) = self.no_hoist(span, "a `match` or `?` on the right of `??`", |g| g.expr(r))?;
        let rc = self.coerce(rc, rt, inner);
        Some((format!("({}.tag == 0 ? {}.u.v0.f_value : ({}))", tmp, tmp, rc), inner))
    }

    /// Test + bindings for a pattern against the value `val` (a C lvalue) of type `ct`.
    fn pattern(&mut self, p: &Pattern, val: &str, ct: CT) -> Option<(String, Vec<(String, String, CT)>)> {
        match &p.kind {
            PatternKind::Wildcard => Some(("1".into(), vec![])),
            PatternKind::Literal(e) => {
                let (lc, lt) = self.expr(e)?;
                let cond = if lt == CT::Str { format!("ioc_streq({}, {})", val, lc) } else { format!("({} == {})", val, lc) };
                Some((cond, vec![]))
            }
            PatternKind::Binding(name) => {
                if let CT::Enum(ei) = ct {
                    if let Some(vi) = self.variant_index(ei, name) {
                        if self.enums[ei].variants[vi].fields.is_empty() {
                            return Some((format!("({}.tag == {})", val, vi), vec![]));
                        }
                    }
                }
                Some(("1".into(), vec![(name.clone(), val.to_string(), ct)]))
            }
            PatternKind::Path(path) | PatternKind::Variant { path, .. } => {
                let CT::Enum(ei) = ct else {
                    self.unsupported(p.span, "this pattern");
                    return None;
                };
                let vname = path.last()?.as_str();
                let Some(vi) = self.variant_index(ei, vname) else {
                    self.err(p.span, format!("no variant `{}` here", vname));
                    return None;
                };
                let mut conds = vec![format!("({}.tag == {})", val, vi)];
                let mut binds = Vec::new();
                if let PatternKind::Variant { args, .. } = &p.kind {
                    let fields = self.enums[ei].variants[vi].fields.clone();
                    for (sub, (fname, fct)) in args.iter().zip(fields.iter()) {
                        let sub_val = format!("{}.u.v{}.f_{}", val, vi, fname);
                        let (c, b) = self.pattern(sub, &sub_val, *fct)?;
                        if c != "1" {
                            conds.push(c);
                        }
                        binds.extend(b);
                    }
                }
                Some((conds.join(" && "), binds))
            }
        }
    }

    /// `match`: used as a statement (`want == false`) or for its value. Statements are emitted in place.
    fn match_gen(&mut self, scrut: &Expr, arms: &[MatchArm], want: bool, span: Span) -> Option<(String, CT)> {
        let (sc, st) = self.expr(scrut)?;
        if matches!(st, CT::Obj(_) | CT::Ref(_) | CT::StrArr | CT::Void) {
            self.unsupported(span, "`match` on this kind of value");
            return None;
        }
        let m = self.fresh("ioc_m");
        let done = self.fresh("ioc_done");
        let res = self.fresh("ioc_res");
        let res_pos = self.out.len();
        let mut res_ct: Option<CT> = if want { self.expect } else { None };
        if let Some(c) = res_ct {
            self.line(&format!("{} {};", c.c(), res));
        }
        self.line("{");
        self.ind += 1;
        self.line(&format!("{} {} = {};", st.c(), m, sc));
        let saved_expect = self.expect;
        for arm in arms {
            self.push_scope();
            let (cond, binds) = self.pattern(&arm.pattern, &m, st)?;
            self.line(&format!("if ({}) {{", cond));
            self.ind += 1;
            for (n, code, ct) in &binds {
                self.line(&format!("{} v_{} = {};", ct.c(), n, code));
                self.declare(n, *ct);
            }
            if let Some(g) = &arm.guard {
                let gc = self.no_hoist(g.span, "a `match` or `?` inside a guard", |x| x.expr(g))?.0;
                self.line(&format!("if ({}) {{", gc));
                self.ind += 1;
            }
            self.expect = res_ct;
            match &arm.body.kind {
                ExprKind::Block(b) => {
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
                ExprKind::Return(v) => match v {
                    Some(e) => {
                        let rt = self.ret;
                        if let Some(val) = self.owned_value_as(e, Some(rt)) {
                            self.emit_return(Some(val));
                        }
                    }
                    None => self.emit_return(None),
                },
                _ if want => {
                    if let Some((c, t)) = self.expr(&arm.body) {
                        if t != CT::Void {
                            let target = match res_ct {
                                Some(r) => r,
                                None => {
                                    res_ct = Some(t);
                                    let decl = format!("{} {};\n", t.c(), res);
                                    self.out.insert_str(res_pos, &decl);
                                    t
                                }
                            };
                            let c = self.coerce(c, t, target);
                            self.line(&format!("{} = {};", res, c));
                        } else {
                            self.line(&format!("{};", c));
                        }
                    }
                }
                _ => {
                    if let Some((c, t)) = self.expr(&arm.body) {
                        self.discard(c, t);
                    }
                }
            }
            self.line(&format!("goto {};", done));
            if arm.guard.is_some() {
                self.ind -= 1;
                self.line("}");
            }
            self.ind -= 1;
            self.line("}");
            self.pop_scope();
        }
        self.expect = saved_expect;
        self.line("ioc_panic(\"no match arm matched\");");
        self.line(&format!("{}:;", done));
        self.ind -= 1;
        self.line("}");
        match (want, res_ct) {
            (true, Some(c)) => Some((res, c)),
            _ => Some(("((void)0)".into(), CT::Void)),
        }
    }

    /// Like `owned_value`, with the type the value will be stored as (for `T?` wrapping and `None`).
    fn owned_value_as(&mut self, e: &Expr, expected: Option<CT>) -> Option<(String, CT)> {
        let saved = self.expect;
        self.expect = expected;
        let r = self.owned_value(e);
        self.expect = saved;
        let (c, t) = r?;
        match expected {
            Some(to) => Some((self.coerce(c, t, to), to.max_with(t))),
            None => Some((c, t)),
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

fn member_span(m: &Member) -> Span {
    match m {
        Member::Method(x) => x.span,
        Member::Field(x) => x.span,
        Member::Property(x) => x.span,
        Member::Constructor(x) => x.span,
        Member::Destructor(x) => x.span,
        Member::Type(x) => x.span,
        Member::Enum(x) => x.span,
    }
}
