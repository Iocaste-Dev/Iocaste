//! Symbol table.
//!
//! Phases, in order:
//!   A. declare:  walk every file and register all types (so order of declaration does not matter)
//!   B. resolve:  turn the written signatures (fields, methods, bases, ...) into semantic types
//!   C. verify:   declaration-level rules (interfaces implemented, `override`, inheritance, ...)
//! Method bodies are checked afterwards by `check.rs`.

use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::ty::*;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// A semantic error together with the index of the file it belongs to.
#[derive(Debug, Clone)]
pub struct SemaError {
    pub file: usize,
    pub diag: Diagnostic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vis {
    Public,
    Internal,
    Protected,
    Private,
}

pub fn vis_from(m: &Modifiers, default: Vis) -> Vis {
    if m.contains(Modifier::Public) {
        Vis::Public
    } else if m.contains(Modifier::Private) {
        Vis::Private
    } else if m.contains(Modifier::Protected) {
        Vis::Protected
    } else if m.contains(Modifier::Internal) {
        Vis::Internal
    } else {
        default
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefKind {
    Class,
    Struct,
    Record,
    Interface,
    Enum,
}

impl DefKind {
    pub fn word(self) -> &'static str {
        match self {
            DefKind::Class => "class",
            DefKind::Struct => "struct",
            DefKind::Record => "record",
            DefKind::Interface => "interface",
            DefKind::Enum => "enum",
        }
    }
}

/// Where a declaration lives: its namespace and the imports visible there.
#[derive(Debug)]
pub struct Scope {
    pub namespace: Vec<String>,
    pub imports: Vec<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct GenParam {
    pub id: ParamId,
    pub name: String,
    pub bounds: Vec<Ty>,
}

#[derive(Debug, Clone)]
pub struct ParamInfo {
    pub name: String,
    pub ty: Ty,
    pub has_default: bool,
}

pub struct FieldInfo<'a> {
    pub name: String,
    pub ty: Ty,
    pub is_static: bool,
    pub readonly: bool,
    pub vis: Vis,
    pub decl: Option<&'a FieldDecl>,
    pub span: Span,
}

pub struct PropInfo<'a> {
    pub name: String,
    pub ty: Ty,
    pub is_static: bool,
    pub vis: Vis,
    pub has_get: bool,
    pub has_set: bool,
    pub set_vis: Vis,
    pub decl: Option<&'a PropertyDecl>,
    pub span: Span,
}

pub struct MethodInfo<'a> {
    pub name: String,
    pub generics: Vec<GenParam>,
    pub params: Vec<ParamInfo>,
    pub ret: Ty,
    pub is_static: bool,
    pub is_abstract: bool,
    pub is_virtual: bool,
    pub is_override: bool,
    pub is_async: bool,
    pub vis: Vis,
    pub decl: Option<&'a MethodDecl>,
    pub span: Span,
}

pub struct CtorInfo<'a> {
    pub params: Vec<ParamInfo>,
    pub vis: Vis,
    pub decl: Option<&'a CtorDecl>,
    pub span: Span,
}

pub struct VariantInfo {
    pub name: String,
    pub fields: Vec<ParamInfo>,
    pub span: Span,
}

#[derive(Clone, Copy)]
pub enum AstDecl<'a> {
    Type(&'a TypeDecl),
    Enum(&'a EnumDecl),
}

pub struct TypeDef<'a> {
    pub name: String,
    pub full_name: String,
    pub kind: DefKind,
    pub vis: Vis,
    pub is_static: bool,
    pub is_abstract: bool,
    pub is_sealed: bool,
    pub parent: Option<TypeId>,
    pub scope: Rc<Scope>,
    pub generics: Vec<GenParam>,
    pub bases: Vec<Ty>,
    pub fields: Vec<FieldInfo<'a>>,
    pub props: Vec<PropInfo<'a>>,
    pub methods: Vec<MethodInfo<'a>>,
    pub ctors: Vec<CtorInfo<'a>>,
    pub dtors: Vec<&'a DtorDecl>,
    pub variants: Vec<VariantInfo>,
    pub nested: HashMap<String, TypeId>,
    pub file: usize,
    pub span: Span,
    pub ast: Option<AstDecl<'a>>,
}

/// Types the compiler itself needs to know about.
#[derive(Debug, Clone)]
pub struct Lang {
    pub option: TypeId,
    pub result: TypeId,
    pub shared: TypeId,
    pub task: TypeId,
    pub iterable: TypeId,
    pub list: TypeId,
}

const NONE: TypeId = usize::MAX;

/// Context needed to resolve a written type name.
pub struct ResolveCtx<'c> {
    pub scope: &'c Scope,
    pub owner: Option<TypeId>,
    pub tparams: &'c [(String, ParamId)],
}

#[derive(Clone)]
pub struct MethodCand {
    /// The type that declares the method, with its generic arguments filled in.
    pub owner_ty: Ty,
    pub owner: TypeId,
    pub idx: usize,
}

pub enum Found {
    Field { owner: TypeId, idx: usize, ty: Ty },
    Prop { owner: TypeId, idx: usize, ty: Ty },
    Methods(Vec<MethodCand>),
    ArrayLength,
}

pub struct Program<'a> {
    pub types: Vec<TypeDef<'a>>,
    pub registry: HashMap<String, TypeId>,
    pub namespaces: HashSet<String>,
    pub param_names: Vec<String>,
    pub param_bounds: HashMap<ParamId, Vec<Ty>>,
    pub prim_defs: HashMap<Prim, TypeId>,
    pub prelude_variants: HashMap<String, (TypeId, usize)>,
    pub lang: Lang,
    pub errors: Vec<SemaError>,
    pub prelude_file: usize,
    imports: Vec<(usize, Vec<String>, Span)>,
}

#[derive(Default)]
struct Built<'a> {
    bases: Vec<Ty>,
    fields: Vec<FieldInfo<'a>>,
    props: Vec<PropInfo<'a>>,
    methods: Vec<MethodInfo<'a>>,
    ctors: Vec<CtorInfo<'a>>,
    dtors: Vec<&'a DtorDecl>,
    variants: Vec<VariantInfo>,
    bounds: Vec<(ParamId, Vec<Ty>)>,
    diags: Vec<Diagnostic>,
}

impl<'a> Program<'a> {
    pub fn new(prelude_file: usize) -> Self {
        Program {
            types: Vec::new(),
            registry: HashMap::new(),
            namespaces: HashSet::new(),
            param_names: Vec::new(),
            param_bounds: HashMap::new(),
            prim_defs: HashMap::new(),
            prelude_variants: HashMap::new(),
            lang: Lang { option: NONE, result: NONE, shared: NONE, task: NONE, iterable: NONE, list: NONE },
            errors: Vec::new(),
            prelude_file,
            imports: Vec::new(),
        }
    }

    fn err(&mut self, file: usize, span: Span, msg: impl Into<String>) {
        self.errors.push(SemaError { file, diag: Diagnostic::new(msg, span) });
    }

    pub fn new_param(&mut self, name: &str) -> GenParam {
        self.param_names.push(name.to_string());
        GenParam { id: (self.param_names.len() - 1) as ParamId, name: name.to_string(), bounds: Vec::new() }
    }

    // ------------------------------------------------------- phase A: declare

    pub fn add_file(&mut self, file: usize, sf: &'a SourceFile) {
        self.declare_items(&sf.items, file, &[], &[]);
    }

    fn register_namespace(&mut self, ns: &[String]) {
        for k in 1..=ns.len() {
            self.namespaces.insert(ns[..k].join("."));
        }
    }

    fn declare_items(&mut self, items: &'a [Item], file: usize, ns: &[String], inherited: &[Vec<String>]) {
        let mut imports: Vec<Vec<String>> = inherited.to_vec();
        for it in items {
            if let Item::Import(i) = it {
                imports.push(i.path.clone());
                self.imports.push((file, i.path.clone(), i.span));
            }
        }
        let scope = Rc::new(Scope { namespace: ns.to_vec(), imports: imports.clone() });
        for it in items {
            match it {
                Item::Import(_) => {}
                Item::Namespace(n) => {
                    let mut nns = ns.to_vec();
                    nns.extend(n.name.iter().cloned());
                    self.register_namespace(&nns);
                    self.declare_items(&n.items, file, &nns, &imports);
                }
                Item::Type(t) => {
                    self.declare_type(t, file, &scope, None);
                }
                Item::Enum(e) => {
                    self.declare_enum(e, file, &scope, None);
                }
            }
        }
    }

    fn full_name(&self, scope: &Scope, parent: Option<TypeId>, name: &str) -> String {
        match parent {
            Some(p) => format!("{}.{}", self.types[p].full_name, name),
            None if scope.namespace.is_empty() => name.to_string(),
            None => format!("{}.{}", scope.namespace.join("."), name),
        }
    }

    fn push_def(&mut self, def: TypeDef<'a>) -> TypeId {
        let id = self.types.len();
        let file = def.file;
        let span = def.span;
        if self.registry.contains_key(&def.full_name) {
            let msg = format!("the {} `{}` is already declared", def.kind.word(), def.full_name);
            self.err(file, span, msg);
        } else {
            self.registry.insert(def.full_name.clone(), id);
        }
        if let Some(p) = def.parent {
            self.types[p].nested.insert(def.name.clone(), id);
        }
        self.types.push(def);
        id
    }

    fn declare_type(&mut self, t: &'a TypeDecl, file: usize, scope: &Rc<Scope>, parent: Option<TypeId>) -> TypeId {
        let generics: Vec<GenParam> = t.generics.iter().map(|g| self.new_param(&g.name)).collect();
        let kind = match t.kind {
            TypeKind::Class => DefKind::Class,
            TypeKind::Struct => DefKind::Struct,
            TypeKind::Record => DefKind::Record,
            TypeKind::Interface => DefKind::Interface,
        };
        let def = TypeDef {
            name: t.name.clone(),
            full_name: self.full_name(scope, parent, &t.name),
            kind,
            vis: vis_from(&t.modifiers, Vis::Internal),
            is_static: t.modifiers.contains(Modifier::Static),
            is_abstract: t.modifiers.contains(Modifier::Abstract) || kind == DefKind::Interface,
            is_sealed: t.modifiers.contains(Modifier::Sealed),
            parent,
            scope: scope.clone(),
            generics,
            bases: Vec::new(),
            fields: Vec::new(),
            props: Vec::new(),
            methods: Vec::new(),
            ctors: Vec::new(),
            dtors: Vec::new(),
            variants: Vec::new(),
            nested: HashMap::new(),
            file,
            span: t.span,
            ast: Some(AstDecl::Type(t)),
        };
        let id = self.push_def(def);
        for m in &t.members {
            match m {
                Member::Type(nt) => {
                    self.declare_type(nt, file, scope, Some(id));
                }
                Member::Enum(ne) => {
                    self.declare_enum(ne, file, scope, Some(id));
                }
                _ => {}
            }
        }
        id
    }

    fn declare_enum(&mut self, e: &'a EnumDecl, file: usize, scope: &Rc<Scope>, parent: Option<TypeId>) -> TypeId {
        let generics: Vec<GenParam> = e.generics.iter().map(|g| self.new_param(&g.name)).collect();
        let def = TypeDef {
            name: e.name.clone(),
            full_name: self.full_name(scope, parent, &e.name),
            kind: DefKind::Enum,
            vis: vis_from(&e.modifiers, Vis::Internal),
            is_static: false,
            is_abstract: false,
            is_sealed: true,
            parent,
            scope: scope.clone(),
            generics,
            bases: Vec::new(),
            fields: Vec::new(),
            props: Vec::new(),
            methods: Vec::new(),
            ctors: Vec::new(),
            dtors: Vec::new(),
            variants: Vec::new(),
            nested: HashMap::new(),
            file,
            span: e.span,
            ast: Some(AstDecl::Enum(e)),
        };
        self.push_def(def)
    }

    // ---------------------------------------------------------------- driver

    /// Run phases A-end, B and C once every file has been added.
    pub fn finish(&mut self) {
        self.init_lang();
        self.check_imports();
        self.resolve_signatures();
        self.verify_declarations();
    }

    fn init_lang(&mut self) {
        let get = |p: &Program, n: &str| p.registry.get(n).copied().unwrap_or(NONE);
        self.lang = Lang {
            option: get(self, "Ioc.Core.Option"),
            result: get(self, "Ioc.Core.Result"),
            shared: get(self, "Ioc.Core.shared"),
            task: get(self, "Ioc.Core.Task"),
            iterable: get(self, "Ioc.Core.IEnumerable"),
            list: get(self, "Ioc.Collections.List"),
        };
        for p in Prim::ALL {
            if let Some(&id) = self.registry.get(&format!("Ioc.Core.{}", p.name())) {
                self.prim_defs.insert(p, id);
            }
        }
        for tid in [self.lang.option, self.lang.result] {
            if tid != NONE {
                // variants are resolved later, but their names are known from the AST
                if let Some(AstDecl::Enum(e)) = self.types[tid].ast {
                    for (i, v) in e.variants.iter().enumerate() {
                        self.prelude_variants.insert(v.name.clone(), (tid, i));
                    }
                }
            }
        }
    }

    fn check_imports(&mut self) {
        let imports = std::mem::take(&mut self.imports);
        for (file, path, span) in imports {
            let full = path.join(".");
            if !self.registry.contains_key(&full) && !self.namespaces.contains(&full) {
                self.errors.push(SemaError {
                    file,
                    diag: Diagnostic::new(format!("unknown namespace or type `{}`", full), span)
                        .with_hint("check the spelling, or add the cluster that provides it to `ioc.toml`"),
                });
            }
        }
    }

    // ----------------------------------------------------- type-name lookup

    /// Generic parameters visible inside type `tid` (outermost first).
    pub fn type_tparams(&self, tid: TypeId) -> Vec<(String, ParamId)> {
        let mut chain = Vec::new();
        let mut cur = Some(tid);
        while let Some(t) = cur {
            chain.push(t);
            cur = self.types[t].parent;
        }
        let mut out = Vec::new();
        for t in chain.into_iter().rev() {
            for g in &self.types[t].generics {
                out.push((g.name.clone(), g.id));
            }
        }
        out
    }

    fn descend(&self, id: TypeId, rest: &[String]) -> Option<TypeId> {
        let mut cur = id;
        for seg in rest {
            cur = *self.types[cur].nested.get(seg)?;
        }
        Some(cur)
    }

    pub fn lookup_type(&self, path: &[String], ctx: &ResolveCtx) -> Option<TypeId> {
        let first = path.first()?;
        // 1. the enclosing types and their nested types
        let mut cur = ctx.owner;
        while let Some(t) = cur {
            if let Some(&n) = self.types[t].nested.get(first) {
                if let Some(r) = self.descend(n, &path[1..]) {
                    return Some(r);
                }
            }
            if &self.types[t].name == first {
                if let Some(r) = self.descend(t, &path[1..]) {
                    return Some(r);
                }
            }
            cur = self.types[t].parent;
        }
        // 2. the namespace chain, innermost first
        let ns = &ctx.scope.namespace;
        for k in (0..=ns.len()).rev() {
            let prefix = ns[..k].join(".");
            let join = |tail: &str| if prefix.is_empty() { tail.to_string() } else { format!("{}.{}", prefix, tail) };
            if let Some(&id) = self.registry.get(&join(&path.join("."))) {
                return Some(id);
            }
            if let Some(&id) = self.registry.get(&join(first)) {
                if let Some(r) = self.descend(id, &path[1..]) {
                    return Some(r);
                }
            }
        }
        // 3. imports (a namespace import brings in its types, a type import that one type)
        for imp in &ctx.scope.imports {
            let base = imp.join(".");
            if let Some(&id) = self.registry.get(&format!("{}.{}", base, path.join("."))) {
                return Some(id);
            }
            if let Some(&id) = self.registry.get(&format!("{}.{}", base, first)) {
                if let Some(r) = self.descend(id, &path[1..]) {
                    return Some(r);
                }
            }
            if let Some(&id) = self.registry.get(&base) {
                if &self.types[id].name == first {
                    if let Some(r) = self.descend(id, &path[1..]) {
                        return Some(r);
                    }
                }
            }
        }
        // 4. the prelude
        if let Some(&id) = self.registry.get(&format!("Ioc.Core.{}", path.join("."))) {
            return Some(id);
        }
        None
    }

    /// Turn a written type into a semantic type. Problems are appended to `errs`.
    pub fn resolve_type(&self, tr: &TypeRef, ctx: &ResolveCtx, errs: &mut Vec<Diagnostic>) -> Ty {
        match &tr.kind {
            TypeRefKind::Named { path, args } => {
                if path.len() == 1 && args.is_empty() {
                    if let Some((_, id)) = ctx.tparams.iter().rev().find(|(n, _)| n == &path[0]) {
                        return Ty::Param(*id);
                    }
                }
                if path.len() == 1 {
                    if path[0] == "void" && args.is_empty() {
                        return Ty::Void;
                    }
                    if let Some(p) = Prim::from_name(&path[0]) {
                        if !args.is_empty() {
                            errs.push(Diagnostic::new(format!("`{}` does not take type arguments", p.name()), tr.span));
                            return Ty::Error;
                        }
                        return Ty::Prim(p);
                    }
                }
                let targs: Vec<Ty> = args.iter().map(|a| self.resolve_type(a, ctx, errs)).collect();
                match self.lookup_type(path, ctx) {
                    Some(id) => {
                        let want = self.types[id].generics.len();
                        if want != targs.len() {
                            let name = path.join(".");
                            let msg = if targs.is_empty() {
                                format!("the type `{}` needs {} type argument(s)", name, want)
                            } else if want == 0 {
                                format!("the type `{}` does not take type arguments", name)
                            } else {
                                format!("the type `{}` takes {} type argument(s) but {} were given", name, want, targs.len())
                            };
                            errs.push(Diagnostic::new(msg, tr.span));
                            return Ty::Error;
                        }
                        Ty::Named { id, args: targs }
                    }
                    None => {
                        errs.push(Diagnostic::new(format!("cannot find the type `{}`", path.join(".")), tr.span));
                        Ty::Error
                    }
                }
            }
            TypeRefKind::Array(inner) => Ty::Array(Box::new(self.resolve_type(inner, ctx, errs))),
            TypeRefKind::Optional(inner) => {
                let t = self.resolve_type(inner, ctx, errs);
                Ty::Named { id: self.lang.option, args: vec![t] }
            }
            TypeRefKind::Ref { mutable, inner } => {
                Ty::Ref { mutable: *mutable, inner: Box::new(self.resolve_type(inner, ctx, errs)) }
            }
        }
    }

    // ------------------------------------------------- phase B: signatures

    fn resolve_params(&self, ps: &[Param], ctx: &ResolveCtx, errs: &mut Vec<Diagnostic>) -> Vec<ParamInfo> {
        ps.iter()
            .map(|p| ParamInfo { name: p.name.clone(), ty: self.resolve_type(&p.ty, ctx, errs), has_default: p.default.is_some() })
            .collect()
    }

    fn resolve_signatures(&mut self) {
        for tid in 0..self.types.len() {
            let Some(ast) = self.types[tid].ast else { continue };
            // allocate ids for the generic parameters of methods first (needs &mut self)
            let mut method_params: Vec<Vec<GenParam>> = Vec::new();
            if let AstDecl::Type(t) = ast {
                for m in &t.members {
                    if let Member::Method(md) = m {
                        let mut v = Vec::new();
                        for g in &md.generics {
                            v.push(self.new_param(&g.name));
                        }
                        method_params.push(v);
                    }
                }
            }
            let built = self.build_members(tid, ast, method_params);
            let file = self.types[tid].file;
            for d in built.diags {
                self.errors.push(SemaError { file, diag: d });
            }
            for (id, bs) in &built.bounds {
                self.param_bounds.insert(*id, bs.clone());
            }
            let def = &mut self.types[tid];
            for g in def.generics.iter_mut() {
                if let Some((_, bs)) = built.bounds.iter().find(|(id, _)| *id == g.id) {
                    g.bounds = bs.clone();
                }
            }
            def.bases = built.bases;
            def.fields = built.fields;
            def.props = built.props;
            def.methods = built.methods;
            def.ctors = built.ctors;
            def.dtors = built.dtors;
            def.variants = built.variants;
        }
    }

    fn build_members(&self, tid: TypeId, ast: AstDecl<'a>, mparams: Vec<Vec<GenParam>>) -> Built<'a> {
        let def = &self.types[tid];
        let mut b = Built::default();
        let base_tp = self.type_tparams(tid);
        let ctx = ResolveCtx { scope: &*def.scope, owner: Some(tid), tparams: &base_tp };

        match ast {
            AstDecl::Enum(e) => {
                for v in &e.variants {
                    let fields = self.resolve_params(&v.fields, &ctx, &mut b.diags);
                    b.variants.push(VariantInfo { name: v.name.clone(), fields, span: v.span });
                }
            }
            AstDecl::Type(t) => {
                for (i, g) in t.generics.iter().enumerate() {
                    let bounds: Vec<Ty> = g.constraints.iter().map(|c| self.resolve_type(c, &ctx, &mut b.diags)).collect();
                    b.bounds.push((def.generics[i].id, bounds));
                }
                for base in &t.bases {
                    let ty = self.resolve_type(base, &ctx, &mut b.diags);
                    match ty {
                        Ty::Named { .. } | Ty::Error => b.bases.push(ty),
                        _ => b.diags.push(Diagnostic::new("a type can only inherit from a class or an interface", base.span)),
                    }
                }
                let is_iface = def.kind == DefKind::Interface;
                let mut mi = 0;
                for m in &t.members {
                    match m {
                        Member::Field(f) => {
                            let ty = self.resolve_type(&f.ty, &ctx, &mut b.diags);
                            if ty == Ty::Void {
                                b.diags.push(Diagnostic::new("a field cannot have type `void`", f.ty.span));
                            }
                            b.fields.push(FieldInfo {
                                name: f.name.clone(),
                                ty,
                                is_static: f.modifiers.contains(Modifier::Static),
                                readonly: f.modifiers.contains(Modifier::Readonly),
                                vis: vis_from(&f.modifiers, Vis::Private),
                                decl: Some(f),
                                span: f.span,
                            });
                        }
                        Member::Property(p) => {
                            let ty = self.resolve_type(&p.ty, &ctx, &mut b.diags);
                            let vis = vis_from(&p.modifiers, if is_iface { Vis::Public } else { Vis::Private });
                            let setter = p.accessors.iter().find(|a| a.kind == AccessorKind::Set);
                            b.props.push(PropInfo {
                                name: p.name.clone(),
                                ty,
                                is_static: p.modifiers.contains(Modifier::Static),
                                vis,
                                has_get: p.accessors.iter().any(|a| a.kind == AccessorKind::Get),
                                has_set: setter.is_some(),
                                set_vis: setter.map(|a| vis_from(&a.modifiers, vis)).unwrap_or(vis),
                                decl: Some(p),
                                span: p.span,
                            });
                        }
                        Member::Method(md) => {
                            let gens = mparams[mi].clone();
                            mi += 1;
                            let mut tp = base_tp.clone();
                            tp.extend(gens.iter().map(|g| (g.name.clone(), g.id)));
                            let mctx = ResolveCtx { scope: &*def.scope, owner: Some(tid), tparams: &tp };
                            let mut gens = gens;
                            for (gi, g) in md.generics.iter().enumerate() {
                                let bounds: Vec<Ty> =
                                    g.constraints.iter().map(|c| self.resolve_type(c, &mctx, &mut b.diags)).collect();
                                gens[gi].bounds = bounds.clone();
                                b.bounds.push((gens[gi].id, bounds));
                            }
                            let params = self.resolve_params(&md.params, &mctx, &mut b.diags);
                            let ret = match &md.ret {
                                Some(r) => self.resolve_type(r, &mctx, &mut b.diags),
                                None => Ty::Void,
                            };
                            let is_abstract = md.modifiers.contains(Modifier::Abstract)
                                || (is_iface && matches!(md.body, Body::None));
                            b.methods.push(MethodInfo {
                                name: md.name.clone(),
                                generics: gens,
                                params,
                                ret,
                                is_static: md.modifiers.contains(Modifier::Static),
                                is_abstract,
                                is_virtual: md.modifiers.contains(Modifier::Virtual),
                                is_override: md.modifiers.contains(Modifier::Override),
                                is_async: md.modifiers.contains(Modifier::Async),
                                vis: vis_from(&md.modifiers, if is_iface { Vis::Public } else { Vis::Private }),
                                decl: Some(md),
                                span: md.span,
                            });
                        }
                        Member::Constructor(c) => {
                            let params = self.resolve_params(&c.params, &ctx, &mut b.diags);
                            b.ctors.push(CtorInfo {
                                params,
                                vis: vis_from(&c.modifiers, Vis::Private),
                                decl: Some(c),
                                span: c.span,
                            });
                        }
                        Member::Destructor(d) => b.dtors.push(d),
                        Member::Type(_) | Member::Enum(_) => {}
                    }
                }
                // records: the primary-constructor parameters become read-only properties + a constructor
                if def.kind == DefKind::Record {
                    let params = self.resolve_params(&t.record_params, &ctx, &mut b.diags);
                    for p in &params {
                        b.props.push(PropInfo {
                            name: p.name.clone(),
                            ty: p.ty.clone(),
                            is_static: false,
                            vis: Vis::Public,
                            has_get: true,
                            has_set: false,
                            set_vis: Vis::Public,
                            decl: None,
                            span: t.span,
                        });
                    }
                    b.ctors.push(CtorInfo { params, vis: Vis::Public, decl: None, span: t.span });
                }
                // classes and structs without a constructor get an implicit parameterless one
                if matches!(def.kind, DefKind::Class | DefKind::Struct) && !def.is_static && b.ctors.is_empty() {
                    b.ctors.push(CtorInfo { params: Vec::new(), vis: Vis::Public, decl: None, span: t.span });
                }
            }
        }
        b
    }

    // ------------------------------------------------------- member queries

    pub fn param_map(&self, def: &TypeDef, args: &[Ty]) -> HashMap<ParamId, Ty> {
        def.generics.iter().zip(args.iter()).map(|(g, a)| (g.id, a.clone())).collect()
    }

    /// All base types of `ty` (transitively), with generic arguments substituted.
    pub fn supertypes(&self, ty: &Ty) -> Vec<Ty> {
        let mut out: Vec<Ty> = Vec::new();
        let mut queue = vec![ty.clone()];
        let mut steps = 0;
        while let Some(t) = queue.pop() {
            steps += 1;
            if steps > 256 {
                break;
            }
            if let Ty::Named { id, args } = &t {
                let def = &self.types[*id];
                let map = self.param_map(def, args);
                for b in &def.bases {
                    let sb = subst(b, &map);
                    if !out.contains(&sb) {
                        out.push(sb.clone());
                        queue.insert(0, sb);
                    }
                }
            }
        }
        out
    }

    /// Find the instance of `target` among the base types of `ty` (or `ty` itself).
    pub fn as_instance_of(&self, ty: &Ty, target: TypeId) -> Option<Ty> {
        if let Ty::Named { id, .. } = ty {
            if *id == target {
                return Some(ty.clone());
            }
        }
        self.supertypes(ty).into_iter().find(|s| matches!(s, Ty::Named { id, .. } if *id == target))
    }

    pub fn find_member(&self, recv: &Ty, name: &str) -> Option<Found> {
        match recv {
            Ty::Prim(p) => {
                let id = *self.prim_defs.get(p)?;
                self.find_in(&Ty::Named { id, args: Vec::new() }, name)
            }
            Ty::Array(_) => {
                if name == "length" {
                    Some(Found::ArrayLength)
                } else {
                    None
                }
            }
            Ty::Named { .. } => self.find_in(recv, name),
            Ty::Param(p) => {
                for b in self.param_bounds.get(p)? {
                    if let Some(f) = self.find_in(b, name) {
                        return Some(f);
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn find_in(&self, ty: &Ty, name: &str) -> Option<Found> {
        let mut chain = vec![ty.clone()];
        chain.extend(self.supertypes(ty));
        let mut methods: Vec<MethodCand> = Vec::new();
        for t in &chain {
            let Ty::Named { id, args } = t else { continue };
            let def = &self.types[*id];
            let map = self.param_map(def, args);
            if methods.is_empty() {
                if let Some((i, f)) = def.fields.iter().enumerate().find(|(_, f)| f.name == name) {
                    return Some(Found::Field { owner: *id, idx: i, ty: subst(&f.ty, &map) });
                }
                if let Some((i, p)) = def.props.iter().enumerate().find(|(_, p)| p.name == name) {
                    return Some(Found::Prop { owner: *id, idx: i, ty: subst(&p.ty, &map) });
                }
            }
            for (i, m) in def.methods.iter().enumerate() {
                if m.name == name {
                    methods.push(MethodCand { owner_ty: t.clone(), owner: *id, idx: i });
                }
            }
        }
        if methods.is_empty() {
            None
        } else {
            Some(Found::Methods(methods))
        }
    }

    pub fn is_option(&self, ty: &Ty) -> Option<Ty> {
        match ty {
            Ty::Named { id, args } if *id == self.lang.option && args.len() == 1 => Some(args[0].clone()),
            _ => None,
        }
    }

    /// Is `inner` the same type as, or nested inside, `outer`?
    pub fn is_inside(&self, inner: TypeId, outer: TypeId) -> bool {
        let mut cur = Some(inner);
        while let Some(t) = cur {
            if t == outer {
                return true;
            }
            cur = self.types[t].parent;
        }
        false
    }

    pub fn show(&self, ty: &Ty) -> String {
        match ty {
            Ty::Error => "{error}".into(),
            Ty::Never => "never".into(),
            Ty::Void => "void".into(),
            Ty::Prim(p) => p.name().into(),
            Ty::Named { id, args } => {
                if let Some(inner) = self.is_option(ty) {
                    return format!("{}?", self.show(&inner));
                }
                let mut s = self.types[*id].name.clone();
                if !args.is_empty() {
                    s.push('<');
                    s.push_str(&args.iter().map(|a| self.show(a)).collect::<Vec<_>>().join(", "));
                    s.push('>');
                }
                s
            }
            Ty::Array(t) => format!("{}[]", self.show(t)),
            Ty::Ref { mutable, inner } => format!("{}ref {}", if *mutable { "mut " } else { "" }, self.show(inner)),
            Ty::Param(p) => self.param_names[*p as usize].clone(),
            Ty::Infer(_) => "_".into(),
        }
    }

    fn sig(&self, name: &str, params: &[Ty], ret: &Ty) -> String {
        format!(
            "{}({}): {}",
            name,
            params.iter().map(|p| self.show(p)).collect::<Vec<_>>().join(", "),
            self.show(ret)
        )
    }

    // -------------------------------------------- phase C: declaration rules

    fn verify_declarations(&mut self) {
        let mut errs: Vec<(usize, Diagnostic)> = Vec::new();
        for tid in 0..self.types.len() {
            if self.types[tid].file == self.prelude_file {
                continue;
            }
            self.verify_type(tid, &mut errs);
        }
        for (file, diag) in errs {
            self.errors.push(SemaError { file, diag });
        }
    }

    pub fn this_ty(&self, tid: TypeId) -> Ty {
        Ty::Named { id: tid, args: self.types[tid].generics.iter().map(|g| Ty::Param(g.id)).collect() }
    }

    fn verify_type(&self, tid: TypeId, errs: &mut Vec<(usize, Diagnostic)>) {
        let def = &self.types[tid];
        let file = def.file;
        let mut push = |span: Span, msg: String, hint: Option<&str>| {
            let mut d = Diagnostic::new(msg, span);
            if let Some(h) = hint {
                d = d.with_hint(h);
            }
            errs.push((file, d));
        };
        let kw = def.kind.word();

        // ---- inheritance rules
        let mut class_bases = 0;
        for b in &def.bases {
            let Ty::Named { id, .. } = b else { continue };
            let bd = &self.types[*id];
            match def.kind {
                DefKind::Interface => {
                    if bd.kind != DefKind::Interface {
                        push(def.span, format!("interface `{}` can only extend interfaces, not {} `{}`", def.name, bd.kind.word(), bd.name), None);
                    }
                }
                DefKind::Struct => {
                    if bd.kind != DefKind::Interface {
                        push(def.span, format!("struct `{}` can only implement interfaces, not inherit from {} `{}`", def.name, bd.kind.word(), bd.name), None);
                    }
                }
                DefKind::Class | DefKind::Record => match bd.kind {
                    DefKind::Interface => {}
                    DefKind::Class | DefKind::Record => {
                        class_bases += 1;
                        if bd.is_sealed {
                            push(def.span, format!("cannot inherit from sealed class `{}`", bd.name), None);
                        }
                        if bd.is_static {
                            push(def.span, format!("cannot inherit from static class `{}`", bd.name), None);
                        }
                    }
                    _ => push(def.span, format!("`{}` cannot inherit from {} `{}`", def.name, bd.kind.word(), bd.name), None),
                },
                DefKind::Enum => {}
            }
        }
        if class_bases > 1 {
            push(def.span, format!("`{}` has more than one base class", def.name), Some("a class can extend one class and implement any number of interfaces"));
        }
        let this = self.this_ty(tid);
        let supers = self.supertypes(&this);
        if supers.iter().any(|s| matches!(s, Ty::Named { id, .. } if *id == tid)) {
            push(def.span, format!("`{}` inherits from itself (cyclic inheritance)", def.name), None);
            return;
        }

        // ---- duplicates
        let mut seen: HashSet<&str> = HashSet::new();
        for f in &def.fields {
            if !seen.insert(&f.name) {
                push(f.span, format!("`{}` is already declared in `{}`", f.name, def.name), None);
            }
        }
        for p in &def.props {
            if !seen.insert(&p.name) {
                push(p.span, format!("`{}` is already declared in `{}`", p.name, def.name), None);
            }
        }
        for (i, m) in def.methods.iter().enumerate() {
            if seen.contains(m.name.as_str()) && !def.methods[..i].iter().any(|o| o.name == m.name) {
                push(m.span, format!("`{}` is already declared in `{}`", m.name, def.name), None);
            }
            for o in &def.methods[..i] {
                if o.name == m.name
                    && o.params.len() == m.params.len()
                    && o.params.iter().zip(&m.params).all(|(a, b)| a.ty == b.ty)
                {
                    push(m.span, format!("`{}` is declared twice with the same parameter types", m.name), None);
                    break;
                }
            }
        }
        let mut vseen: HashSet<&str> = HashSet::new();
        for v in &def.variants {
            if !vseen.insert(&v.name) {
                push(v.span, format!("variant `{}` is declared twice in enum `{}`", v.name, def.name), None);
            }
        }

        // ---- static classes
        if def.is_static {
            for f in def.fields.iter().filter(|f| !f.is_static) {
                push(f.span, format!("static class `{}` cannot have the instance field `{}`", def.name, f.name), Some("add `static`"));
            }
            for m in def.methods.iter().filter(|m| !m.is_static) {
                push(m.span, format!("static class `{}` cannot have the instance method `{}`", def.name, m.name), Some("add `static`"));
            }
            for p in def.props.iter().filter(|p| !p.is_static) {
                push(p.span, format!("static class `{}` cannot have the instance property `{}`", def.name, p.name), Some("add `static`"));
            }
        }

        // ---- method modifiers and bodies
        for m in &def.methods {
            let has_body = !matches!(m.decl.map(|d| &d.body), Some(Body::None) | None);
            if m.is_abstract && has_body && def.kind != DefKind::Interface {
                push(m.span, format!("abstract method `{}` cannot have a body", m.name), None);
            }
            if m.decl.map(|d| d.modifiers.contains(Modifier::Abstract)).unwrap_or(false) && !def.is_abstract {
                push(m.span, format!("abstract method `{}` in non-abstract {} `{}`", m.name, kw, def.name), Some("mark the class `abstract`"));
            }
            if !has_body && !m.is_abstract && def.kind != DefKind::Interface {
                push(m.span, format!("method `{}` has no body", m.name), Some("add `{ ... }` or `=> expression;`"));
            }
            if m.is_override {
                let found = supers.iter().any(|s| {
                    let Ty::Named { id, args } = s else { return false };
                    let sd = &self.types[*id];
                    if sd.kind == DefKind::Interface {
                        return false;
                    }
                    let map = self.param_map(sd, args);
                    sd.methods.iter().any(|o| {
                        o.name == m.name
                            && (o.is_virtual || o.is_abstract || o.is_override)
                            && o.params.len() == m.params.len()
                            && o.params.iter().zip(&m.params).all(|(a, b)| subst(&a.ty, &map) == b.ty)
                    })
                });
                if !found {
                    push(m.span, format!("`{}` is marked `override` but there is no virtual method to override", m.name), Some("the base class method must be `virtual` or `abstract` and have the same parameters"));
                }
            }
        }

        // ---- everything a concrete type must implement
        let concrete = matches!(def.kind, DefKind::Class | DefKind::Struct | DefKind::Record) && !def.is_abstract;
        if concrete {
            // implementation candidates: this type and its non-interface bases
            let mut chain = vec![this.clone()];
            chain.extend(supers.iter().filter(|s| matches!(s, Ty::Named { id, .. } if self.types[*id].kind != DefKind::Interface)).cloned());

            for s in &supers {
                let Ty::Named { id, args } = s else { continue };
                let sd = &self.types[*id];
                let map = self.param_map(sd, args);
                let is_iface = sd.kind == DefKind::Interface;
                let is_abstract_class = sd.kind == DefKind::Class && sd.is_abstract;
                if !is_iface && !is_abstract_class {
                    continue;
                }
                for m in sd.methods.iter().filter(|m| m.is_abstract) {
                    if !m.generics.is_empty() {
                        continue;
                    }
                    let want: Vec<Ty> = m.params.iter().map(|p| subst(&p.ty, &map)).collect();
                    let want_ret = subst(&m.ret, &map);
                    let implemented = chain.iter().any(|c| {
                        let Ty::Named { id: cid, args: cargs } = c else { return false };
                        let cd = &self.types[*cid];
                        let cmap = self.param_map(cd, cargs);
                        cd.methods.iter().any(|o| {
                            !o.is_abstract
                                && o.name == m.name
                                && o.params.len() == want.len()
                                && o.params.iter().zip(&want).all(|(a, w)| &subst(&a.ty, &cmap) == w)
                                && subst(&o.ret, &cmap) == want_ret
                        })
                    });
                    if !implemented {
                        push(
                            def.span,
                            format!("{} `{}` does not implement `{}.{}`", kw, def.name, sd.name, self.sig(&m.name, &want, &want_ret)),
                            Some("add a matching `public` method (same name, parameter types and return type)"),
                        );
                    }
                }
                if is_iface {
                    for p in &sd.props {
                        let want_ty = subst(&p.ty, &map);
                        let implemented = chain.iter().any(|c| {
                            let Ty::Named { id: cid, args: cargs } = c else { return false };
                            let cd = &self.types[*cid];
                            let cmap = self.param_map(cd, cargs);
                            cd.props.iter().any(|o| {
                                o.name == p.name && subst(&o.ty, &cmap) == want_ty && (!p.has_get || o.has_get) && (!p.has_set || o.has_set)
                            })
                        });
                        if !implemented {
                            push(
                                def.span,
                                format!("{} `{}` does not implement the property `{}.{}: {}`", kw, def.name, sd.name, p.name, self.show(&want_ty)),
                                None,
                            );
                        }
                    }
                }
            }
        }
    }
}
