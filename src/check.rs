//! Body checker: name resolution and type checking of everything inside method bodies.
//!
//! Types are inferred with unification (generic calls like `Ok(x)` or `new List<T>()` work
//! without annotations). Errors are collected, not fatal: after the first problem in an
//! expression the checker uses `Ty::Error`, which silences follow-up errors.

use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::symbols::*;
use crate::ty::*;
use std::collections::{HashMap, HashSet};

pub fn check_bodies(prog: &Program) -> Vec<SemaError> {
    let mut out = Vec::new();
    for tid in 0..prog.types.len() {
        let def = &prog.types[tid];
        if def.file == prog.prelude_file {
            continue;
        }
        for m in &def.methods {
            if let Some(d) = m.decl {
                let mut c = Checker::new(prog, tid, m.is_static);
                c.method(m, d);
                out.extend(c.into_errors());
            }
        }
        for p in &def.props {
            if let Some(d) = p.decl {
                let mut c = Checker::new(prog, tid, p.is_static);
                c.property(p, d);
                out.extend(c.into_errors());
            }
        }
        for f in &def.fields {
            if let Some(d) = f.decl {
                if let Some(init) = &d.init {
                    let mut c = Checker::new(prog, tid, f.is_static);
                    c.push_scope();
                    let t = c.expr(init, Some(&f.ty));
                    c.coerce_at(init.span, &t, &f.ty);
                    out.extend(c.into_errors());
                }
            }
        }
        for ct in &def.ctors {
            if let Some(d) = ct.decl {
                let mut c = Checker::new(prog, tid, false);
                c.in_ctor = true;
                c.ctor(ct, d);
                out.extend(c.into_errors());
            }
        }
        for d in &def.dtors {
            let mut c = Checker::new(prog, tid, false);
            c.ret.push(RetCtx { ret: Ty::Void });
            c.block(&d.body);
            out.extend(c.into_errors());
        }
    }
    out
}

struct RetCtx {
    ret: Ty,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LocalKind {
    Let,
    Var,
    Param,
}

struct Local {
    id: usize,
    name: String,
    ty: Ty,
    kind: LocalKind,
    deferred: bool,
}

#[derive(Clone)]
struct Root {
    name: String,
    declared_mut: bool,
}

#[derive(Clone)]
enum ValKind {
    Local { kind: LocalKind, deferred: bool },
    Field { readonly: bool },
    Prop { has_set: bool, set_vis: Vis, owner: TypeId },
    Elem,
    Other,
}

/// A value-producing expression that may also be a place (variable, field, element).
#[derive(Clone)]
struct Val {
    ty: Ty,
    /// reached through an immutable reference
    ro: bool,
    /// the local variable this place lives in (None when reached through a `mut ref`)
    root: Option<Root>,
    kind: ValKind,
    name: String,
}

impl Val {
    fn plain(ty: Ty) -> Val {
        Val { ty, ro: false, root: None, kind: ValKind::Other, name: "value".into() }
    }
}

enum Res {
    Val(Val),
    Type(TypeId),
    Namespace(String),
    Methods { name: String, recv: Option<Ty>, implicit: bool, base_call: bool, cands: Vec<MethodCand> },
    Variant { enum_id: TypeId, idx: usize },
    Error,
}

#[derive(Clone)]
struct Cand {
    params: Vec<(Ty, bool)>,
    generics: Vec<ParamId>,
    ret: Ty,
    is_static: bool,
    vis: Vis,
    owner: TypeId,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MutKind {
    Assign,
    Update,
    Borrow,
}

enum PatCover {
    All,
    Variant(usize),
    Bool(bool),
    Nothing,
}

struct Checker<'p, 'a> {
    prog: &'p Program<'a>,
    tid: TypeId,
    file: usize,
    diags: Vec<Diagnostic>,
    tparams: Vec<(String, ParamId)>,
    is_static: bool,
    in_ctor: bool,
    is_async: bool,
    ret: Vec<RetCtx>,
    scopes: Vec<Vec<Local>>,
    infer: Vec<Option<Ty>>,
    loops: usize,
    all_locals: Vec<(String, Ty, Span)>,
    // ---- ownership tracking (v0.0.4)
    next_id: usize,
    moved: HashMap<usize, Span>,
    loans: Vec<Loan>,
    uses: HashMap<String, Vec<(u32, u32)>>,
    uses_ready: bool,
    skip_moved: bool,
}

/// `let r = ref x;` / `let r = mut ref x;`: `holder` keeps `target` borrowed while `holder` is still used.
#[derive(Clone)]
struct Loan {
    holder_id: usize,
    holder: String,
    target: usize,
    mutable: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    Move,
    Shared,
    Mut,
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur.push((prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost));
        }
        prev = cur;
    }
    prev[b.len()]
}

fn suggest(name: &str, cands: &[String]) -> Option<String> {
    let max = (name.chars().count() / 3).max(1);
    cands
        .iter()
        .filter(|c| c.as_str() != name)
        .map(|c| (edit_distance(name, c), c))
        .filter(|(d, _)| *d <= max)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c.clone())
}

fn stmt_diverges(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Return(_) => true,
        StmtKind::Block(b) | StmtKind::Unsafe(b) => b.stmts.iter().any(stmt_diverges),
        StmtKind::If { then, otherwise, .. } => match otherwise {
            Some(o) => stmt_diverges(then) && stmt_diverges(o),
            None => false,
        },
        StmtKind::Expr(e) => expr_diverges(e),
        _ => false,
    }
}

fn expr_diverges(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Return(_) => true,
        ExprKind::Block(b) => b.stmts.iter().any(stmt_diverges),
        ExprKind::Match { arms, .. } => !arms.is_empty() && arms.iter().all(|a| expr_diverges(&a.body)),
        _ => false,
    }
}


/// Does this statement always leave the surrounding block (return / break / continue)?
fn stmt_exits(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue => true,
        StmtKind::Block(b) | StmtKind::Unsafe(b) => b.stmts.iter().any(stmt_exits),
        StmtKind::If { then, otherwise, .. } => match otherwise {
            Some(o) => stmt_exits(then) && stmt_exits(o),
            None => false,
        },
        StmtKind::Expr(e) => expr_diverges(e),
        _ => false,
    }
}

/// The variable at the root of a place expression: `a.b[0].c` -> `a`.
fn root_ident(e: &Expr) -> Option<&str> {
    match &e.kind {
        ExprKind::Ident(n) => Some(n),
        ExprKind::Member { target, .. } | ExprKind::Index { target, .. } => root_ident(target),
        _ => None,
    }
}

fn collect_block(b: &Block, out: &mut Vec<(String, u32, u32)>) {
    for s in &b.stmts {
        collect_stmt(s, out);
    }
}

fn collect_stmt(s: &Stmt, out: &mut Vec<(String, u32, u32)>) {
    match &s.kind {
        StmtKind::Local { init, .. } => {
            if let Some(e) = init {
                collect_expr(e, out);
            }
        }
        StmtKind::Expr(e) => collect_expr(e, out),
        StmtKind::Block(b) | StmtKind::Unsafe(b) => collect_block(b, out),
        StmtKind::If { cond, then, otherwise } => {
            collect_expr(cond, out);
            collect_stmt(then, out);
            if let Some(o) = otherwise {
                collect_stmt(o, out);
            }
        }
        StmtKind::While { cond, body } => {
            collect_expr(cond, out);
            collect_stmt(body, out);
        }
        StmtKind::For { init, cond, update, body } => {
            if let Some(i) = init {
                collect_stmt(i, out);
            }
            if let Some(c) = cond {
                collect_expr(c, out);
            }
            if let Some(u) = update {
                collect_expr(u, out);
            }
            collect_stmt(body, out);
        }
        StmtKind::ForIn { iter, body, .. } => {
            collect_expr(iter, out);
            collect_stmt(body, out);
        }
        StmtKind::Return(v) => {
            if let Some(e) = v {
                collect_expr(e, out);
            }
        }
        StmtKind::Break | StmtKind::Continue => {}
    }
}

fn collect_expr(e: &Expr, out: &mut Vec<(String, u32, u32)>) {
    match &e.kind {
        ExprKind::Ident(n) => out.push((n.clone(), e.span.line, e.span.col)),
        ExprKind::Interp(segs) => {
            for s in segs {
                if let InterpSegment::Expr(x) = s {
                    collect_expr(x, out);
                }
            }
        }
        ExprKind::Member { target, .. } => collect_expr(target, out),
        ExprKind::Index { target, index } => {
            collect_expr(target, out);
            collect_expr(index, out);
        }
        ExprKind::Call { callee, args } => {
            collect_expr(callee, out);
            args.iter().for_each(|a| collect_expr(a, out));
        }
        ExprKind::New { args, .. } => args.iter().for_each(|a| collect_expr(a, out)),
        ExprKind::Unary { expr, .. } | ExprKind::Postfix { expr, .. } | ExprKind::Borrow { expr, .. } => collect_expr(expr, out),
        ExprKind::Try(x) | ExprKind::Await(x) => collect_expr(x, out),
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_expr(lhs, out);
            collect_expr(rhs, out);
        }
        ExprKind::Assign { target, value, .. } => {
            collect_expr(target, out);
            collect_expr(value, out);
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_expr(scrutinee, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    collect_expr(g, out);
                }
                collect_expr(&a.body, out);
            }
        }
        ExprKind::Spawn(b) | ExprKind::Block(b) => collect_block(b, out),
        ExprKind::Return(v) => {
            if let Some(x) = v {
                collect_expr(x, out);
            }
        }
        ExprKind::Array(items) => items.iter().for_each(|a| collect_expr(a, out)),
        _ => {}
    }
}

fn block_diverges(b: &Block) -> bool {
    b.stmts.iter().any(stmt_diverges)
}

impl<'p, 'a> Checker<'p, 'a> {
    fn new(prog: &'p Program<'a>, tid: TypeId, is_static: bool) -> Self {
        Checker {
            prog,
            tid,
            file: prog.types[tid].file,
            diags: Vec::new(),
            tparams: prog.type_tparams(tid),
            is_static,
            in_ctor: false,
            is_async: false,
            ret: Vec::new(),
            scopes: vec![Vec::new()],
            infer: Vec::new(),
            loops: 0,
            all_locals: Vec::new(),
            next_id: 0,
            moved: HashMap::new(),
            loans: Vec::new(),
            uses: HashMap::new(),
            uses_ready: false,
            skip_moved: false,
        }
    }

    fn into_errors(mut self) -> Vec<SemaError> {
        for (name, ty, span) in std::mem::take(&mut self.all_locals) {
            if self.resolve(&ty).has_infer() {
                self.diags.push(
                    Diagnostic::new(format!("cannot infer the type of `{}`", name), span)
                        .with_hint("add a type annotation, e.g. `let x: Type = ...`"),
                );
            }
        }
        let file = self.file;
        let mut seen: HashSet<(u32, u32, String)> = HashSet::new();
        self.diags
            .into_iter()
            .filter(|d| seen.insert((d.span.line, d.span.col, d.message.clone())))
            .map(|diag| SemaError { file, diag })
            .collect()
    }

    // ------------------------------------------------------------- helpers

    fn err(&mut self, span: Span, msg: impl Into<String>) {
        self.diags.push(Diagnostic::new(msg, span));
    }

    fn err_hint(&mut self, span: Span, msg: impl Into<String>, hint: impl Into<String>) {
        self.diags.push(Diagnostic::new(msg, span).with_hint(hint));
    }

    fn show(&self, t: &Ty) -> String {
        self.prog.show(&self.resolve(t))
    }

    fn this_ty(&self) -> Ty {
        self.prog.this_ty(self.tid)
    }

    fn resolve_ty(&mut self, tr: &TypeRef) -> Ty {
        let prog = self.prog;
        let scope = prog.types[self.tid].scope.clone();
        let ctx = ResolveCtx { scope: &scope, owner: Some(self.tid), tparams: &self.tparams };
        prog.resolve_type(tr, &ctx, &mut self.diags)
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        if let Some(sc) = self.scopes.pop() {
            let ids: Vec<usize> = sc.iter().map(|l| l.id).collect();
            self.loans.retain(|l| !ids.contains(&l.holder_id));
        }
    }

    fn declare(&mut self, name: &str, ty: Ty, kind: LocalKind, deferred: bool, span: Span) {
        if name == "_" {
            return;
        }
        if self.scopes.last().map_or(false, |s| s.iter().any(|l| l.name == name)) {
            self.err_hint(
                span,
                format!("`{}` is already declared in this scope", name),
                "use another name, or assign to the existing variable if it was declared with `var`",
            );
        }
        if kind != LocalKind::Param {
            self.all_locals.push((name.to_string(), ty.clone(), span));
        }
        if self.scopes.is_empty() {
            self.scopes.push(Vec::new());
        }
        let id = self.next_id;
        self.next_id += 1;
        self.scopes.last_mut().unwrap().push(Local { id, name: name.to_string(), ty, kind, deferred });
    }


    // ------------------------------------------------ ownership (v0.0.4)

    fn find_local(&self, name: &str) -> Option<(usize, Ty)> {
        for scope in self.scopes.iter().rev() {
            if let Some(l) = scope.iter().rev().find(|l| l.name == name) {
                return Some((l.id, l.ty.clone()));
            }
        }
        None
    }

    fn local_name(&self, id: usize) -> String {
        for scope in &self.scopes {
            if let Some(l) = scope.iter().find(|l| l.id == id) {
                return l.name.clone();
            }
        }
        "value".into()
    }

    /// Values that are copied instead of moved.
    fn is_copy(&self, t: &Ty) -> bool {
        let prog = self.prog;
        match self.resolve(t) {
            Ty::Prim(_) | Ty::Ref { .. } | Ty::Void | Ty::Never | Ty::Error | Ty::Infer(_) => true,
            Ty::Param(_) | Ty::Array(_) => false,
            Ty::Named { id, args } => {
                let def = &prog.types[id];
                match def.kind {
                    DefKind::Struct => true,
                    DefKind::Enum => {
                        if id == prog.lang.option || id == prog.lang.result {
                            args.iter().all(|a| self.is_copy(a))
                        } else {
                            def.variants.iter().all(|v| v.fields.iter().all(|f| self.is_copy(&f.ty)))
                        }
                    }
                    _ => false,
                }
            }
        }
    }

    /// Is `name` mentioned in the source after `at`? (a cheap form of "the borrow is still alive")
    fn used_after(&self, name: &str, at: Span) -> bool {
        self.uses.get(name).map_or(false, |v| v.iter().any(|&(l, c)| (l, c) > (at.line, at.col)))
    }

    /// A borrow of `target` that is still alive at `at` and conflicts with an access.
    /// `exclusive`: the access writes, moves or mutably borrows (conflicts with every loan);
    /// otherwise it only reads (conflicts with mutable loans).
    fn live_loan(&self, target: usize, at: Span, exclusive: bool) -> Option<Loan> {
        self.loans
            .iter()
            .find(|l| l.target == target && (exclusive || l.mutable) && self.used_after(&l.holder, at))
            .cloned()
    }

    fn note_use(&mut self, name: &str, id: usize, span: Span) {
        if !self.skip_moved {
            if let Some(at) = self.moved.get(&id).copied() {
                self.err_hint(
                    span,
                    format!("use of moved value `{}`", name),
                    format!(
                        "`{}` was moved at line {}:{}. Pass `ref {}` to lend it, or `{}.clone()` on a `shared<T>` to copy the handle",
                        name, at.line, at.col, name, name
                    ),
                );
                return;
            }
            if let Some(l) = self.live_loan(id, span, false) {
                let msg = format!("cannot use `{}` while it is mutably borrowed by `{}`", name, l.holder);
                self.err_hint(span, msg, format!("`{}` is used again later; finish with `{}` first, or end the borrow in a smaller `{{ }}` block", l.holder, name));
            }
        }
    }

    /// The expression's value is given away (assigned, passed by value, returned...).
    fn consume(&mut self, e: &Expr) {
        let ExprKind::Ident(name) = &e.kind else { return };
        let Some((id, ty)) = self.find_local(name) else { return };
        if self.is_copy(&ty) {
            return;
        }
        if let Some(l) = self.live_loan(id, e.span, true) {
            let msg = format!("cannot move `{}` because it is borrowed by `{}`", name, l.holder);
            self.err_hint(e.span, msg, format!("`{}` is used again later; finish using it first", l.holder));
        }
        self.moved.insert(id, e.span);
    }

    fn consume_args(&mut self, recv: Option<&Expr>, args: &[Expr]) {
        let mut uses: Vec<(usize, Access, Span, String)> = Vec::new();
        if let Some(Expr { kind: ExprKind::Ident(n), span }) = recv {
            if let Some((id, _)) = self.find_local(n) {
                uses.push((id, Access::Shared, *span, n.clone()));
            }
        }
        for a in args {
            match &a.kind {
                ExprKind::Ident(n) => {
                    if let Some((id, ty)) = self.find_local(n) {
                        if !self.is_copy(&ty) {
                            uses.push((id, Access::Move, a.span, n.clone()));
                        }
                    }
                }
                ExprKind::Borrow { mutable, expr } => {
                    if let ExprKind::Ident(n) = &expr.kind {
                        if let Some((id, _)) = self.find_local(n) {
                            uses.push((id, if *mutable { Access::Mut } else { Access::Shared }, a.span, n.clone()));
                        }
                    }
                }
                _ => {}
            }
        }
        'outer: for i in 0..uses.len() {
            for j in 0..i {
                if uses[i].0 == uses[j].0 && (uses[i].1 != Access::Shared || uses[j].1 != Access::Shared) {
                    let msg = format!("`{}` is used more than once in this call, and one use moves it or borrows it mutably", uses[i].3);
                    self.err_hint(uses[i].2, msg, "pass it once, or make a copy/`shared<T>` handle for the other use");
                    break 'outer;
                }
            }
        }
        for a in args {
            self.consume(a);
        }
    }

    fn merge_states(&mut self, before: HashMap<usize, Span>, states: Vec<HashMap<usize, Span>>) {
        if states.is_empty() {
            self.moved = before;
            return;
        }
        let mut out: HashMap<usize, Span> = HashMap::new();
        for st in states {
            for (k, v) in st {
                out.entry(k).or_insert(v);
            }
        }
        self.moved = out;
    }

    /// Run a loop body. Anything declared outside the loop that the body moves would be moved
    /// again on the next iteration.
    fn loop_body<F: FnOnce(&mut Self)>(&mut self, body: &Stmt, f: F) {
        let before = self.moved.clone();
        let start = self.next_id;
        f(self);
        let after = self.moved.clone();
        if !stmt_exits(body) {
            let mut fresh: Vec<(usize, Span)> =
                after.iter().filter(|(k, _)| !before.contains_key(k) && **k < start).map(|(k, v)| (*k, *v)).collect();
            fresh.sort_by_key(|(_, sp)| (sp.line, sp.col));
            for (id, sp) in fresh {
                let n = self.local_name(id);
                self.err_hint(
                    sp,
                    format!("`{}` is moved inside a loop, so it would be moved again on the next iteration", n),
                    format!("lend it with `ref {}`, or give `{}` a new value before the loop repeats", n, n),
                );
            }
        }
        let mut merged = before;
        for (k, v) in after {
            merged.entry(k).or_insert(v);
        }
        self.moved = merged;
    }

    // ----------------------------------------------------------- inference

    fn fresh(&mut self) -> Ty {
        self.infer.push(None);
        Ty::Infer((self.infer.len() - 1) as u32)
    }

    fn shallow(&self, ty: &Ty) -> Ty {
        let mut t = ty.clone();
        while let Ty::Infer(i) = t {
            match &self.infer[i as usize] {
                Some(n) => t = n.clone(),
                None => break,
            }
        }
        t
    }

    fn resolve(&self, ty: &Ty) -> Ty {
        match self.shallow(ty) {
            Ty::Named { id, args } => Ty::Named { id, args: args.iter().map(|a| self.resolve(a)).collect() },
            Ty::Array(t) => Ty::Array(Box::new(self.resolve(&t))),
            Ty::Ref { mutable, inner } => Ty::Ref { mutable, inner: Box::new(self.resolve(&inner)) },
            t => t,
        }
    }

    /// Resolve and strip any `ref` / `mut ref` layers.
    fn peel(&self, t: &Ty) -> Ty {
        let mut r = self.resolve(t);
        loop {
            match &r {
                Ty::Ref { inner, .. } => {
                    let n = (**inner).clone();
                    r = n;
                }
                _ => break,
            }
        }
        r
    }

    fn occurs(&self, v: u32, ty: &Ty) -> bool {
        match self.shallow(ty) {
            Ty::Infer(i) => i == v,
            Ty::Named { args, .. } => args.iter().any(|a| self.occurs(v, a)),
            Ty::Array(t) => self.occurs(v, &t),
            Ty::Ref { inner, .. } => self.occurs(v, &inner),
            _ => false,
        }
    }

    fn unify(&mut self, a: &Ty, b: &Ty) -> bool {
        let a = self.shallow(a);
        let b = self.shallow(b);
        match (&a, &b) {
            (Ty::Error, _) | (_, Ty::Error) => true,
            (Ty::Infer(x), Ty::Infer(y)) if x == y => true,
            (Ty::Infer(x), _) => {
                if self.occurs(*x, &b) {
                    false
                } else {
                    self.infer[*x as usize] = Some(b.clone());
                    true
                }
            }
            (_, Ty::Infer(y)) => {
                if self.occurs(*y, &a) {
                    false
                } else {
                    self.infer[*y as usize] = Some(a.clone());
                    true
                }
            }
            (Ty::Never, _) | (_, Ty::Never) => true,
            (Ty::Prim(p), Ty::Prim(q)) => p == q,
            (Ty::Void, Ty::Void) => true,
            (Ty::Param(x), Ty::Param(y)) => x == y,
            (Ty::Array(x), Ty::Array(y)) => self.unify(x, y),
            (Ty::Ref { mutable: m1, inner: i1 }, Ty::Ref { mutable: m2, inner: i2 }) => m1 == m2 && self.unify(i1, i2),
            (Ty::Named { id: i1, args: a1 }, Ty::Named { id: i2, args: a2 }) => {
                if i1 != i2 || a1.len() != a2.len() {
                    return false;
                }
                for (x, y) in a1.iter().zip(a2.iter()) {
                    if !self.unify(x, y) {
                        return false;
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// Like `unify` but allows `a` to be a subtype (a derived class / implemented interface).
    fn coerce_obj(&mut self, a: &Ty, b: &Ty) -> bool {
        let a = self.shallow(a);
        let b = self.shallow(b);
        if let (Ty::Named { id: x, .. }, Ty::Named { id: y, .. }) = (&a, &b) {
            if x != y {
                if let Some(inst) = self.prog.as_instance_of(&a, *y) {
                    return self.unify(&inst, &b);
                }
                return false;
            }
        }
        self.unify(&a, &b)
    }

    /// Can a value of type `from` be used where `to` is expected?
    fn coerce(&mut self, from: &Ty, to: &Ty) -> bool {
        let from = self.shallow(from);
        let to = self.shallow(to);
        match (&from, &to) {
            (Ty::Error, _) | (_, Ty::Error) | (Ty::Never, _) => return true,
            (Ty::Infer(_), _) | (_, Ty::Infer(_)) => return self.unify(&from, &to),
            (Ty::Prim(a), Ty::Prim(b)) => return a.widens_to(*b),
            (Ty::Ref { mutable: m1, inner: i1 }, Ty::Ref { mutable: m2, inner: i2 }) => {
                if !*m1 && *m2 {
                    return false;
                }
                return self.coerce_obj(i1, i2);
            }
            _ => {}
        }
        if let (Ty::Named { id: a, .. }, Ty::Named { id: b, .. }) = (&from, &to) {
            if a == b {
                return self.unify(&from, &to);
            }
            if let Some(inst) = self.prog.as_instance_of(&from, *b) {
                return self.unify(&inst, &to);
            }
        }
        // T -> T? (implicit wrap)
        if let Some(inner) = self.prog.is_option(&to) {
            if self.prog.is_option(&from).is_none() {
                return self.coerce(&from, &inner);
            }
        }
        self.unify(&from, &to)
    }

    fn mismatch(&mut self, span: Span, expected: &Ty, found: &Ty) {
        let e = self.resolve(expected);
        let f = self.resolve(found);
        if e.has_error() || f.has_error() {
            return;
        }
        let msg = format!("mismatched types: expected `{}`, found `{}`", self.prog.show(&e), self.prog.show(&f));
        let hint = match (&e, &f) {
            (Ty::Ref { mutable, .. }, t) if !matches!(t, Ty::Ref { .. }) => {
                Some(format!("borrow it explicitly: `{} value`", if *mutable { "mut ref" } else { "ref" }))
            }
            (Ty::Ref { mutable: true, .. }, Ty::Ref { mutable: false, .. }) => {
                Some("this is an immutable reference; a `mut ref` is required".to_string())
            }
            (t, Ty::Ref { .. }) if !matches!(t, Ty::Ref { .. }) => {
                Some("a reference cannot be used where an owned value is expected".to_string())
            }
            (Ty::Prim(a), Ty::Prim(b)) if a.is_numeric() && b.is_numeric() => {
                Some("there is no implicit narrowing conversion; the value would lose precision".to_string())
            }
            (t, o) if self.prog.is_option(o).is_some() && self.prog.is_option(t).is_none() => {
                Some("the value is optional (`T?`); unwrap it with `??` or `match`".to_string())
            }
            _ => None,
        };
        match hint {
            Some(h) => self.err_hint(span, msg, h),
            None => self.err(span, msg),
        }
    }

    fn coerce_at(&mut self, span: Span, found: &Ty, expected: &Ty) -> bool {
        if self.coerce(found, expected) {
            true
        } else {
            self.mismatch(span, expected, found);
            false
        }
    }

    fn no_void(&mut self, t: &Ty, span: Span) -> Ty {
        if matches!(self.shallow(t), Ty::Void) {
            self.err(span, "this expression has type `void`, so it has no value to use");
            Ty::Error
        } else {
            t.clone()
        }
    }

    fn check_access(&mut self, vis: Vis, owner: TypeId, span: Span, what: &str) {
        let prog = self.prog;
        let ok = match vis {
            Vis::Public | Vis::Internal => true,
            Vis::Private => prog.is_inside(self.tid, owner),
            Vis::Protected => {
                prog.is_inside(self.tid, owner)
                    || prog.supertypes(&self.this_ty()).iter().any(|s| matches!(s, Ty::Named { id, .. } if *id == owner))
            }
        };
        if !ok {
            let word = if vis == Vis::Private { "private" } else { "protected" };
            self.err(span, format!("{} is {} and cannot be used from here", what, word));
        }
    }

    // ----------------------------------------------------- members to check

    fn declare_params(&mut self, params: &[ParamInfo], decl: &[Param]) {
        for (i, p) in params.iter().enumerate() {
            let span = decl.get(i).map(|d| d.span).unwrap_or_default();
            self.declare(&p.name, p.ty.clone(), LocalKind::Param, false, span);
        }
        for (p, d) in params.iter().zip(decl.iter()) {
            if let Some(def) = &d.default {
                let t = self.expr(def, Some(&p.ty));
                self.coerce_at(def.span, &t, &p.ty);
            }
        }
    }

    fn method(&mut self, m: &MethodInfo, d: &MethodDecl) {
        self.tparams.extend(m.generics.iter().map(|g| (g.name.clone(), g.id)));
        self.is_async = m.is_async;
        let mut ret = m.ret.clone();
        if m.is_async {
            match &m.ret {
                Ty::Named { id, args } if *id == self.prog.lang.task && args.len() == 1 => ret = args[0].clone(),
                Ty::Error => ret = Ty::Error,
                _ => {
                    self.err_hint(m.span, "an `async` method must return `Task<T>`", "for example `async fetch(): Task<string>`");
                    ret = Ty::Error;
                }
            }
        }
        self.ret.push(RetCtx { ret: ret.clone() });
        self.push_scope();
        self.declare_params(&m.params, &d.params);
        match &d.body {
            Body::None => {}
            Body::Expr(e) => {
                let t = self.expr(e, Some(&ret));
                if ret != Ty::Void {
                    self.coerce_at(e.span, &t, &ret);
                }
            }
            Body::Block(b) => {
                self.block(b);
                if ret != Ty::Void && !ret.has_error() && !block_diverges(b) {
                    self.err_hint(
                        m.span,
                        format!("not all code paths of `{}` return a value", m.name),
                        format!("the method returns `{}`; end every path with `return ...;`", self.prog.show(&ret)),
                    );
                }
            }
        }
        self.pop_scope();
    }

    fn ctor(&mut self, c: &CtorInfo, d: &CtorDecl) {
        self.ret.push(RetCtx { ret: Ty::Void });
        self.push_scope();
        self.declare_params(&c.params, &d.params);
        self.block(&d.body);
        self.pop_scope();
    }

    fn property(&mut self, p: &PropInfo, d: &PropertyDecl) {
        for a in &d.accessors {
            self.ret.clear();
            self.scopes = vec![Vec::new()];
            match a.kind {
                AccessorKind::Get => {
                    self.ret.push(RetCtx { ret: p.ty.clone() });
                    match &a.body {
                        Body::Expr(e) => {
                            let t = self.expr(e, Some(&p.ty));
                            self.coerce_at(e.span, &t, &p.ty);
                        }
                        Body::Block(b) => {
                            self.block(b);
                            if !block_diverges(b) {
                                self.err(a.span, "the `get` accessor must return a value on all code paths");
                            }
                        }
                        Body::None => {}
                    }
                }
                AccessorKind::Set => {
                    self.ret.push(RetCtx { ret: Ty::Void });
                    self.declare("value", p.ty.clone(), LocalKind::Param, false, a.span);
                    match &a.body {
                        Body::Expr(e) => {
                            self.expr(e, None);
                        }
                        Body::Block(b) => self.block(b),
                        Body::None => {}
                    }
                }
            }
        }
        if let Some(init) = &d.init {
            self.scopes = vec![Vec::new()];
            let t = self.expr(init, Some(&p.ty));
            self.coerce_at(init.span, &t, &p.ty);
        }
    }

    // ---------------------------------------------------------- statements

    fn block(&mut self, b: &Block) {
        if !self.uses_ready {
            self.uses_ready = true;
            let mut u = Vec::new();
            collect_block(b, &mut u);
            for (n, l, c) in u {
                self.uses.entry(n).or_default().push((l, c));
            }
        }
        self.push_scope();
        for s in &b.stmts {
            self.stmt(s);
        }
        self.pop_scope();
    }

    fn sub_stmt(&mut self, s: &Stmt) {
        self.push_scope();
        self.stmt(s);
        self.pop_scope();
    }

    fn cond(&mut self, e: &Expr) {
        let t = self.expr(e, Some(&Ty::Prim(Prim::Bool)));
        let r = self.peel(&t);
        if !matches!(r, Ty::Prim(Prim::Bool) | Ty::Error | Ty::Never) {
            self.mismatch(e.span, &Ty::Prim(Prim::Bool), &r);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Local { mutable, name, ty, init } => self.local(*mutable, name, ty, init, s.span),
            StmtKind::Expr(e) => {
                self.expr(e, None);
            }
            StmtKind::Block(b) | StmtKind::Unsafe(b) => self.block(b),
            StmtKind::If { cond, then, otherwise } => {
                self.cond(cond);
                let before = self.moved.clone();
                self.sub_stmt(then);
                let mut states: Vec<HashMap<usize, Span>> = Vec::new();
                if !stmt_exits(then) {
                    states.push(self.moved.clone());
                }
                self.moved = before.clone();
                match otherwise {
                    Some(o) => {
                        self.sub_stmt(o);
                        if !stmt_exits(o) {
                            states.push(self.moved.clone());
                        }
                    }
                    None => states.push(before.clone()),
                }
                self.merge_states(before, states);
            }
            StmtKind::While { cond, body } => {
                self.cond(cond);
                self.loop_body(body, |c| {
                    c.loops += 1;
                    c.sub_stmt(body);
                    c.loops -= 1;
                });
            }
            StmtKind::For { init, cond, update, body } => {
                self.push_scope();
                if let Some(i) = init {
                    self.stmt(i);
                }
                if let Some(c) = cond {
                    self.cond(c);
                }
                if let Some(u) = update {
                    self.expr(u, None);
                }
                self.loop_body(body, |c| {
                    c.loops += 1;
                    c.sub_stmt(body);
                    c.loops -= 1;
                });
                self.pop_scope();
            }
            StmtKind::ForIn { mutable, name, iter, body } => {
                let it = self.expr(iter, None);
                let r = self.peel(&it);
                let prog = self.prog;
                let elem = match &r {
                    Ty::Array(t) => (**t).clone(),
                    Ty::Named { .. } => match prog.as_instance_of(&r, prog.lang.iterable) {
                        Some(Ty::Named { args, .. }) if args.len() == 1 => args[0].clone(),
                        _ => {
                            let msg = format!("`{}` cannot be used in a `for ... in` loop", prog.show(&r));
                            self.err_hint(iter.span, msg, "loop over an array, a `List<T>`, a `Channel<T>` or another `IEnumerable<T>`");
                            Ty::Error
                        }
                    },
                    Ty::Error => Ty::Error,
                    _ => {
                        let msg = format!("`{}` cannot be used in a `for ... in` loop", prog.show(&r));
                        self.err(iter.span, msg);
                        Ty::Error
                    }
                };
                self.push_scope();
                self.declare(name, elem, if *mutable { LocalKind::Var } else { LocalKind::Let }, false, s.span);
                self.loop_body(body, |c| {
                    c.loops += 1;
                    c.sub_stmt(body);
                    c.loops -= 1;
                });
                self.pop_scope();
            }
            StmtKind::Return(v) => self.check_return(v.as_ref(), s.span),
            StmtKind::Break | StmtKind::Continue => {
                if self.loops == 0 {
                    let w = if matches!(s.kind, StmtKind::Break) { "break" } else { "continue" };
                    self.err(s.span, format!("`{}` can only be used inside a loop", w));
                }
            }
        }
    }

    fn local(&mut self, mutable: bool, name: &str, ty: &Option<TypeRef>, init: &Option<Expr>, span: Span) {
        let declared = ty.as_ref().map(|t| self.resolve_ty(t));
        let final_ty = match (&declared, init) {
            (Some(t), Some(e)) => {
                let it = self.expr(e, Some(t));
                self.coerce_at(e.span, &it, t);
                t.clone()
            }
            (None, Some(e)) => {
                let it = self.expr(e, None);
                self.no_void(&it, e.span)
            }
            (Some(t), None) => t.clone(),
            (None, None) => Ty::Error,
        };
        if let Some(e) = init {
            self.consume(e);
        }
        self.declare(name, final_ty, if mutable { LocalKind::Var } else { LocalKind::Let }, init.is_none(), span);
        if let Some(Expr { kind: ExprKind::Borrow { mutable: m, expr }, .. }) = init {
            if let ExprKind::Ident(t) = &expr.kind {
                if let (Some((tid, _)), Some((hid, _))) = (self.find_local(t), self.find_local(name)) {
                    self.loans.push(Loan { holder_id: hid, holder: name.to_string(), target: tid, mutable: *m });
                }
            }
        }
    }

    fn check_return(&mut self, v: Option<&Expr>, span: Span) {
        let ret = self.ret.last().map(|c| c.ret.clone()).unwrap_or(Ty::Void);
        match v {
            Some(x) => {
                if ret == Ty::Void {
                    self.err_hint(x.span, "cannot return a value from a method that returns `void`", "remove the value, or give the method a return type");
                    self.expr(x, None);
                } else {
                    let t = self.expr(x, Some(&ret));
                    self.coerce_at(x.span, &t, &ret);
                    self.consume(x);
                    if let (Ty::Ref { .. }, ExprKind::Borrow { expr, .. }) = (self.resolve(&ret), &x.kind) {
                        if let ExprKind::Ident(n) = &expr.kind {
                            if let Some((_, ty)) = self.find_local(n) {
                                if !matches!(self.resolve(&ty), Ty::Ref { .. }) {
                                    self.err_hint(
                                        x.span,
                                        format!("cannot return a reference to the local variable `{}`", n),
                                        "it is freed when the method ends; return the value itself (move it out) instead",
                                    );
                                }
                            }
                        }
                    }
                }
            }
            None => {
                if ret != Ty::Void && !ret.has_error() {
                    let msg = format!("this method must return a value of type `{}`", self.prog.show(&ret));
                    self.err(span, msg);
                }
            }
        }
    }

    // --------------------------------------------------------- expressions

    fn int_lit(&mut self, suffix: &str, exp: Option<&Ty>) -> Ty {
        match suffix.to_ascii_lowercase().as_str() {
            "l" => return Ty::Prim(Prim::Long),
            "u" => return Ty::Prim(Prim::UInt),
            "ul" | "lu" => return Ty::Prim(Prim::ULong),
            _ => {}
        }
        if let Some(x) = exp {
            let r = self.resolve(x);
            if let Ty::Prim(p) = &r {
                if p.is_numeric() {
                    return r;
                }
            }
        }
        Ty::Prim(Prim::Int)
    }

    fn float_lit(&mut self, suffix: &str, exp: Option<&Ty>) -> Ty {
        match suffix.to_ascii_lowercase().as_str() {
            "f" => return Ty::Prim(Prim::Float),
            "d" => return Ty::Prim(Prim::Double),
            _ => {}
        }
        if let Some(x) = exp {
            let r = self.resolve(x);
            if let Ty::Prim(p) = &r {
                if p.is_float() {
                    return r;
                }
            }
        }
        Ty::Prim(Prim::Double)
    }

    fn expr(&mut self, e: &Expr, exp: Option<&Ty>) -> Ty {
        match &e.kind {
            ExprKind::Int { suffix, .. } => self.int_lit(suffix, exp),
            ExprKind::Float { suffix, .. } => self.float_lit(suffix, exp),
            ExprKind::Str(_) => Ty::Prim(Prim::String),
            ExprKind::Interp(segs) => {
                for s in segs {
                    if let InterpSegment::Expr(x) = s {
                        let t = self.expr(x, None);
                        self.no_void(&t, x.span);
                    }
                }
                Ty::Prim(Prim::String)
            }
            ExprKind::Char(_) => Ty::Prim(Prim::Char),
            ExprKind::Bool(_) => Ty::Prim(Prim::Bool),
            ExprKind::Ident(_) | ExprKind::Member { .. } | ExprKind::This | ExprKind::Index { .. } => {
                let r = self.classify(e);
                self.res_value(r, e.span)
            }
            ExprKind::Base => {
                self.err_hint(e.span, "`base` cannot be used on its own", "use it to reach a member of the base class: `base.name`");
                Ty::Error
            }
            ExprKind::Call { callee, args } => {
                let t = self.call(callee, args, e.span, exp);
                let recv = match &callee.kind {
                    ExprKind::Member { target, .. } => Some(&**target),
                    _ => None,
                };
                self.consume_args(recv, args);
                t
            }
            ExprKind::New { ty, args } => {
                let t = self.new_expr(ty, args, e.span);
                self.consume_args(None, args);
                t
            }
            ExprKind::Unary { op, expr } => self.unary(*op, expr, exp),
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, e.span),
            ExprKind::Assign { op, target, value } => self.assign(*op, target, value, e.span),
            ExprKind::Postfix { expr, .. } => self.postfix(expr),
            ExprKind::Try(x) => self.try_expr(x, e.span),
            ExprKind::Await(x) => self.await_expr(x, e.span),
            ExprKind::Borrow { mutable, expr } => self.borrow(*mutable, expr),
            ExprKind::Match { scrutinee, arms } => self.match_expr(scrutinee, arms, e.span, exp),
            ExprKind::Spawn(b) => {
                self.ret.push(RetCtx { ret: Ty::Void });
                let saved = self.loops;
                self.loops = 0;
                self.block(b);
                self.loops = saved;
                self.ret.pop();
                Ty::Named { id: self.prog.lang.task, args: vec![Ty::Void] }
            }
            ExprKind::Block(b) => {
                self.block(b);
                if block_diverges(b) {
                    Ty::Never
                } else {
                    Ty::Void
                }
            }
            ExprKind::Return(v) => {
                self.check_return(v.as_deref(), e.span);
                Ty::Never
            }
            ExprKind::Array(items) => {
                let hint = exp.and_then(|x| match self.shallow(x) {
                    Ty::Array(t) => Some(*t),
                    _ => None,
                });
                let elem = match hint {
                    Some(h) => h,
                    None => self.fresh(),
                };
                for it in items {
                    let t = self.expr(it, Some(&elem));
                    self.coerce_at(it.span, &t, &elem);
                    self.consume(it);
                }
                Ty::Array(Box::new(elem))
            }
        }
    }

    /// Turn a classification into a plain value type, reporting misuse.
    fn res_value(&mut self, r: Res, span: Span) -> Ty {
        match r {
            Res::Val(v) => v.ty,
            Res::Error => Ty::Error,
            Res::Type(id) => {
                let msg = format!("`{}` is a type, not a value", self.prog.types[id].name);
                self.err(span, msg);
                Ty::Error
            }
            Res::Namespace(p) => {
                self.err(span, format!("`{}` is a namespace, not a value", p));
                Ty::Error
            }
            Res::Methods { name, .. } => {
                self.err_hint(span, format!("`{}` is a method, not a value", name), "call it with parentheses: `name(...)`");
                Ty::Error
            }
            Res::Variant { enum_id, idx } => self.variant_value(enum_id, idx, span),
        }
    }

    fn variant_value(&mut self, enum_id: TypeId, idx: usize, span: Span) -> Ty {
        let prog = self.prog;
        let def = &prog.types[enum_id];
        let v = &def.variants[idx];
        if !v.fields.is_empty() {
            let msg = format!("the variant `{}` carries {} value(s) and must be called like `{}(...)`", v.name, v.fields.len(), v.name);
            self.err(span, msg);
            return Ty::Error;
        }
        let args: Vec<Ty> = def.generics.iter().map(|_| self.fresh()).collect();
        Ty::Named { id: enum_id, args }
    }

    // ------------------------------------------------ names and member access

    fn type_names_in_scope(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for s in &self.scopes {
            for l in s {
                v.push(l.name.clone());
            }
        }
        let this = self.this_ty();
        v.extend(self.member_names(&this));
        v
    }

    fn member_names(&self, ty: &Ty) -> Vec<String> {
        let mut out = Vec::new();
        let mut chain = vec![ty.clone()];
        chain.extend(self.prog.supertypes(ty));
        for t in &chain {
            let t = match t {
                Ty::Prim(p) => match self.prog.prim_defs.get(p) {
                    Some(&id) => Ty::Named { id, args: Vec::new() },
                    None => continue,
                },
                other => other.clone(),
            };
            if let Ty::Named { id, .. } = t {
                let d = &self.prog.types[id];
                out.extend(d.fields.iter().map(|f| f.name.clone()));
                out.extend(d.props.iter().map(|p| p.name.clone()));
                out.extend(d.methods.iter().map(|m| m.name.clone()));
            }
        }
        out
    }

    fn classify(&mut self, e: &Expr) -> Res {
        match &e.kind {
            ExprKind::Ident(name) => self.classify_ident(name, e.span),
            ExprKind::This => {
                if self.is_static {
                    self.err(e.span, "`this` cannot be used in a static method");
                    Res::Error
                } else {
                    Res::Val(Val { ty: self.this_ty(), ro: false, root: None, kind: ValKind::Other, name: "this".into() })
                }
            }
            ExprKind::Member { target, name } => self.classify_member(target, name, e.span),
            ExprKind::Index { target, index } => self.classify_index(target, index, e.span),
            _ => {
                let ty = self.expr(e, None);
                Res::Val(Val::plain(ty))
            }
        }
    }

    fn member_val(&mut self, f: &Found, name: &str, span: Span, ro: bool, root: Option<Root>) -> Option<(Val, bool)> {
        let prog = self.prog;
        match f {
            Found::Field { owner, idx, ty } => {
                let fi = &prog.types[*owner].fields[*idx];
                self.check_access(fi.vis, *owner, span, &format!("the field `{}`", name));
                Some((
                    Val { ty: ty.clone(), ro, root, kind: ValKind::Field { readonly: fi.readonly }, name: name.into() },
                    fi.is_static,
                ))
            }
            Found::Prop { owner, idx, ty } => {
                let pi = &prog.types[*owner].props[*idx];
                self.check_access(pi.vis, *owner, span, &format!("the property `{}`", name));
                Some((
                    Val {
                        ty: ty.clone(),
                        ro,
                        root,
                        kind: ValKind::Prop { has_set: pi.has_set, set_vis: pi.set_vis, owner: *owner },
                        name: name.into(),
                    },
                    pi.is_static,
                ))
            }
            Found::ArrayLength => Some((
                Val { ty: Ty::Prim(Prim::Int), ro: true, root: None, kind: ValKind::Other, name: "length".into() },
                false,
            )),
            Found::Methods(_) => None,
        }
    }

    fn classify_ident(&mut self, name: &str, span: Span) -> Res {
        // 1. local variables and parameters
        for scope in self.scopes.iter().rev() {
            if let Some(l) = scope.iter().rev().find(|l| l.name == name) {
                let lid = l.id;
                let res = Res::Val(Val {
                    ty: l.ty.clone(),
                    ro: false,
                    root: Some(Root { name: name.into(), declared_mut: l.kind == LocalKind::Var }),
                    kind: ValKind::Local { kind: l.kind, deferred: l.deferred },
                    name: name.into(),
                });
                self.note_use(name, lid, span);
                return res;
            }
        }
        let prog = self.prog;
        // 2. members of this type (and of the types it is nested in)
        let mut cur = Some(self.tid);
        let mut first = true;
        while let Some(t) = cur {
            let tt = prog.this_ty(t);
            if let Some(f) = prog.find_member(&tt, name) {
                match f {
                    Found::Methods(cands) => {
                        return Res::Methods { name: name.into(), recv: Some(tt), implicit: true, base_call: false, cands };
                    }
                    other => {
                        if let Some((val, is_static)) = self.member_val(&other, name, span, false, None) {
                            if !is_static && (self.is_static || !first) {
                                self.err_hint(
                                    span,
                                    format!("cannot use the instance member `{}` from a static method", name),
                                    "make the member `static`, or call this from an instance method",
                                );
                                return Res::Error;
                            }
                            return Res::Val(val);
                        }
                    }
                }
            }
            cur = prog.types[t].parent;
            first = false;
        }
        // 3. type names
        let scope = prog.types[self.tid].scope.clone();
        if let Some(p) = Prim::from_name(name) {
            if let Some(&id) = prog.prim_defs.get(&p) {
                return Res::Type(id);
            }
        }
        {
            let ctx = ResolveCtx { scope: &scope, owner: Some(self.tid), tparams: &self.tparams };
            if let Some(id) = prog.lookup_type(&[name.to_string()], &ctx) {
                return Res::Type(id);
            }
        }
        // 4. built-in variants: Some, None, Ok, Err
        if let Some(&(enum_id, idx)) = prog.prelude_variants.get(name) {
            return Res::Variant { enum_id, idx };
        }
        // 5. namespaces
        let ns = &scope.namespace;
        for k in (0..=ns.len()).rev() {
            let mut full = ns[..k].join(".");
            if !full.is_empty() {
                full.push('.');
            }
            full.push_str(name);
            if prog.namespaces.contains(&full) {
                return Res::Namespace(full);
            }
        }
        let names = self.type_names_in_scope();
        let msg = format!("cannot find `{}` in this scope", name);
        match suggest(name, &names) {
            Some(s) => self.err_hint(span, msg, format!("did you mean `{}`?", s)),
            None => self.err(span, msg),
        }
        Res::Error
    }

    fn classify_member(&mut self, target: &Expr, name: &str, span: Span) -> Res {
        if let ExprKind::Base = target.kind {
            return self.classify_base(name, span);
        }
        match self.classify(target) {
            Res::Error => Res::Error,
            Res::Val(v) => self.member_of_value(v, name, span),
            Res::Type(id) => self.member_of_type(id, name, span),
            Res::Namespace(p) => {
                let full = format!("{}.{}", p, name);
                if let Some(&id) = self.prog.registry.get(&full) {
                    Res::Type(id)
                } else if self.prog.namespaces.contains(&full) {
                    Res::Namespace(full)
                } else {
                    self.err(span, format!("the namespace `{}` has no type or namespace `{}`", p, name));
                    Res::Error
                }
            }
            Res::Methods { name: m, .. } => {
                self.err_hint(span, format!("`{}` is a method and has no members", m), "call it first: `name(...).member`");
                Res::Error
            }
            Res::Variant { enum_id, idx } => {
                let ty = self.variant_value(enum_id, idx, span);
                if matches!(ty, Ty::Error) {
                    return Res::Error;
                }
                self.member_of_value(Val::plain(ty), name, span)
            }
        }
    }

    fn classify_base(&mut self, name: &str, span: Span) -> Res {
        if self.is_static {
            self.err(span, "`base` cannot be used in a static method");
            return Res::Error;
        }
        let prog = self.prog;
        let this = self.this_ty();
        let base = prog
            .supertypes(&this)
            .into_iter()
            .find(|s| matches!(s, Ty::Named { id, .. } if prog.types[*id].kind != DefKind::Interface));
        let Some(base) = base else {
            self.err(span, "`base` is used but this class has no base class");
            return Res::Error;
        };
        match prog.find_member(&base, name) {
            Some(Found::Methods(cands)) => Res::Methods { name: name.into(), recv: Some(this), implicit: false, base_call: true, cands },
            Some(f) => match self.member_val(&f, name, span, false, None) {
                Some((val, _)) => Res::Val(val),
                None => Res::Error,
            },
            None => {
                let msg = format!("the base class `{}` has no member `{}`", prog.show(&base), name);
                self.err(span, msg);
                Res::Error
            }
        }
    }

    fn member_of_value(&mut self, v: Val, name: &str, span: Span) -> Res {
        let prog = self.prog;
        let mut ty = self.resolve(&v.ty);
        let mut ro = v.ro;
        let mut root = v.root.clone();
        let found;
        loop {
            match &ty {
                Ty::Ref { mutable, inner } => {
                    if !*mutable {
                        ro = true;
                    }
                    root = None;
                    let n = (**inner).clone();
                    ty = self.resolve(&n);
                }
                Ty::Named { id, args } if *id == prog.lang.shared => {
                    if let Some(f) = prog.find_member(&ty, name) {
                        found = Some(f);
                        break;
                    }
                    ro = true;
                    let n = args[0].clone();
                    ty = self.resolve(&n);
                }
                Ty::Infer(_) => {
                    self.err(span, "type annotations needed: the type of this value is not known yet");
                    return Res::Error;
                }
                Ty::Error | Ty::Never => return Res::Error,
                Ty::Void => {
                    self.err(span, "`void` has no members");
                    return Res::Error;
                }
                _ => {
                    found = prog.find_member(&ty, name);
                    break;
                }
            }
        }
        let Some(f) = found else {
            let msg = format!("`{}` has no member named `{}`", prog.show(&ty), name);
            match suggest(name, &self.member_names(&ty)) {
                Some(s) => self.err_hint(span, msg, format!("did you mean `{}`?", s)),
                None => self.err(span, msg),
            }
            return Res::Error;
        };
        match f {
            Found::Methods(cands) => Res::Methods { name: name.into(), recv: Some(ty), implicit: false, base_call: false, cands },
            other => match self.member_val(&other, name, span, ro, root) {
                Some((val, is_static)) => {
                    if is_static {
                        self.err_hint(
                            span,
                            format!("`{}` is static and cannot be reached through a value", name),
                            format!("write `{}.{}`", prog.show(&ty), name),
                        );
                    }
                    Res::Val(val)
                }
                None => Res::Error,
            },
        }
    }

    fn member_of_type(&mut self, id: TypeId, name: &str, span: Span) -> Res {
        let prog = self.prog;
        let def = &prog.types[id];
        if def.kind == DefKind::Enum {
            if let Some(i) = def.variants.iter().position(|v| v.name == name) {
                return Res::Variant { enum_id: id, idx: i };
            }
        }
        if let Some(&n) = def.nested.get(name) {
            return Res::Type(n);
        }
        let args: Vec<Ty> = def.generics.iter().map(|_| self.fresh()).collect();
        let tt = Ty::Named { id, args };
        match prog.find_member(&tt, name) {
            Some(Found::Methods(cands)) => Res::Methods { name: name.into(), recv: None, implicit: false, base_call: false, cands },
            Some(f) => match self.member_val(&f, name, span, false, None) {
                Some((val, is_static)) => {
                    if !is_static {
                        self.err_hint(
                            span,
                            format!("`{}` is an instance member and needs a value", name),
                            format!("create a `{}` first and use it on that value", def.name),
                        );
                        return Res::Error;
                    }
                    Res::Val(val)
                }
                None => Res::Error,
            },
            None => {
                let msg = format!("`{}` has no static member or variant named `{}`", def.name, name);
                match suggest(name, &self.member_names(&tt)) {
                    Some(s) => self.err_hint(span, msg, format!("did you mean `{}`?", s)),
                    None => self.err(span, msg),
                }
                Res::Error
            }
        }
    }

    fn classify_index(&mut self, target: &Expr, index: &Expr, span: Span) -> Res {
        let tv = match self.classify(target) {
            Res::Val(v) => v,
            Res::Error => {
                self.expr(index, None);
                return Res::Error;
            }
            other => {
                let t = self.res_value(other, target.span);
                Val::plain(t)
            }
        };
        let it = self.expr(index, Some(&Ty::Prim(Prim::Int)));
        let ir = self.peel(&it);
        match &ir {
            Ty::Prim(p) if p.is_integer() => {}
            Ty::Error | Ty::Never => {}
            Ty::Infer(_) => {
                self.unify(&ir, &Ty::Prim(Prim::Int));
            }
            other => {
                let msg = format!("an index must be an integer, found `{}`", self.prog.show(other));
                self.err(index.span, msg);
            }
        }
        let mut ty = self.resolve(&tv.ty);
        let mut ro = tv.ro;
        let mut root = tv.root.clone();
        loop {
            match &ty {
                Ty::Ref { mutable, inner } => {
                    if !*mutable {
                        ro = true;
                    }
                    root = None;
                    let n = (**inner).clone();
                    ty = self.resolve(&n);
                }
                _ => break,
            }
        }
        let prog = self.prog;
        let elem = match &ty {
            Ty::Array(t) => (**t).clone(),
            Ty::Prim(Prim::String) => {
                ro = true;
                Ty::Prim(Prim::Char)
            }
            Ty::Named { id, args } if *id == prog.lang.list => args[0].clone(),
            Ty::Error => Ty::Error,
            other => {
                let msg = format!("`{}` cannot be indexed", prog.show(other));
                self.err(span, msg);
                Ty::Error
            }
        };
        Res::Val(Val { ty: elem, ro, root, kind: ValKind::Elem, name: "element".into() })
    }

    // -------------------------------------------------------------- calls

    fn call(&mut self, callee: &Expr, args: &[Expr], span: Span, exp: Option<&Ty>) -> Ty {
        match self.classify(callee) {
            Res::Methods { name, recv, implicit, base_call, cands } => {
                self.call_methods(&name, recv, implicit, base_call, cands, args, span, exp)
            }
            Res::Variant { enum_id, idx } => self.call_variant(enum_id, idx, args, span, exp),
            Res::Val(v) => {
                let msg = format!("cannot call a value of type `{}`", self.show(&v.ty));
                self.err_hint(span, msg, "functions are not first-class values yet (lambdas arrive in a later version)");
                for a in args {
                    self.expr(a, None);
                }
                Ty::Error
            }
            Res::Type(id) => {
                let name = self.prog.types[id].name.clone();
                self.err_hint(span, format!("`{}` is a type, not a function", name), format!("create an instance with `new {}(...)`", name));
                for a in args {
                    self.expr(a, None);
                }
                Ty::Error
            }
            Res::Namespace(p) => {
                self.err(span, format!("`{}` is a namespace, not a function", p));
                Ty::Error
            }
            Res::Error => {
                for a in args {
                    self.expr(a, None);
                }
                Ty::Error
            }
        }
    }

    fn instantiate(&mut self, c: &Cand) -> (Vec<Ty>, Ty) {
        let mut map = HashMap::new();
        for g in &c.generics {
            let v = self.fresh();
            map.insert(*g, v);
        }
        (c.params.iter().map(|p| subst(&p.0, &map)).collect(), subst(&c.ret, &map))
    }

    /// Choose among `cands` for the given arguments and check them. Returns (return type, index).
    fn resolve_call(&mut self, name: &str, cands: &[Cand], args: &[Expr], span: Span, exp: Option<&Ty>) -> Option<(Ty, usize)> {
        let n = args.len();
        let ok: Vec<usize> = (0..cands.len())
            .filter(|&i| {
                let ps = &cands[i].params;
                let required = ps.iter().filter(|p| !p.1).count();
                n >= required && n <= ps.len()
            })
            .collect();
        if ok.is_empty() {
            let prog = self.prog;
            let msg = if cands.len() == 1 {
                let c = &cands[0];
                let required = c.params.iter().filter(|p| !p.1).count();
                let want = if required == c.params.len() { format!("{}", required) } else { format!("{} to {}", required, c.params.len()) };
                format!("`{}` takes {} argument(s) but {} were supplied", name, want, n)
            } else {
                format!("no version of `{}` takes {} argument(s)", name, n)
            };
            let hint = cands
                .iter()
                .map(|c| format!("{}({})", name, c.params.iter().map(|p| prog.show(&p.0)).collect::<Vec<_>>().join(", ")))
                .collect::<Vec<_>>()
                .join("  |  ");
            self.err_hint(span, msg, format!("expected: {}", hint));
            for a in args {
                self.expr(a, None);
            }
            return None;
        }
        if ok.len() == 1 {
            let i = ok[0];
            let (params, ret) = self.instantiate(&cands[i]);
            if let Some(e) = exp {
                if !matches!(self.shallow(&ret), Ty::Void) {
                    let snap = self.infer.clone();
                    if !self.coerce_obj(&ret, e) {
                        self.infer = snap;
                    }
                }
            }
            for (a, p) in args.iter().zip(params.iter()) {
                let t = self.expr(a, Some(p));
                self.coerce_at(a.span, &t, p);
            }
            return Some((ret, i));
        }
        // several candidates: type the arguments on their own, then pick the first that fits
        let tys: Vec<Ty> = args.iter().map(|a| self.expr(a, None)).collect();
        for &i in &ok {
            let snap = self.infer.clone();
            let (params, ret) = self.instantiate(&cands[i]);
            let mut fits = true;
            for (t, p) in tys.iter().zip(params.iter()) {
                if !self.coerce(t, p) {
                    fits = false;
                    break;
                }
            }
            if fits {
                return Some((ret, i));
            }
            self.infer = snap;
        }
        let shown = tys.iter().map(|t| self.show(t)).collect::<Vec<_>>().join(", ");
        let prog = self.prog;
        let hint = cands
            .iter()
            .map(|c| format!("{}({})", name, c.params.iter().map(|p| prog.show(&p.0)).collect::<Vec<_>>().join(", ")))
            .collect::<Vec<_>>()
            .join("  |  ");
        self.err_hint(span, format!("no version of `{}` accepts the arguments ({})", name, shown), format!("available: {}", hint));
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn call_methods(
        &mut self,
        name: &str,
        recv: Option<Ty>,
        implicit: bool,
        base_call: bool,
        cands: Vec<MethodCand>,
        args: &[Expr],
        span: Span,
        exp: Option<&Ty>,
    ) -> Ty {
        let prog = self.prog;
        let mut list: Vec<Cand> = Vec::new();
        for c in &cands {
            let od = &prog.types[c.owner];
            let m = &od.methods[c.idx];
            let oargs = if let Ty::Named { args, .. } = &c.owner_ty { args.clone() } else { Vec::new() };
            let map = prog.param_map(od, &oargs);
            let params: Vec<(Ty, bool)> = m.params.iter().map(|p| (subst(&p.ty, &map), p.has_default)).collect();
            if list.iter().any(|o| o.params.iter().map(|p| &p.0).eq(params.iter().map(|p| &p.0))) {
                continue; // an override of a method we already have
            }
            list.push(Cand {
                params,
                generics: m.generics.iter().map(|g| g.id).collect(),
                ret: subst(&m.ret, &map),
                is_static: m.is_static,
                vis: m.vis,
                owner: c.owner,
            });
        }
        let Some((ret, i)) = self.resolve_call(name, &list, args, span, exp) else { return Ty::Error };
        let c = list[i].clone();
        self.check_access(c.vis, c.owner, span, &format!("the method `{}`", name));
        if c.is_static {
            if recv.is_some() && !implicit && !base_call {
                self.err_hint(
                    span,
                    format!("`{}` is static and cannot be called on a value", name),
                    format!("call it through the type: `{}.{}(...)`", prog.types[c.owner].name, name),
                );
            }
        } else if recv.is_none() {
            self.err_hint(span, format!("`{}` is an instance method and needs a value to call it on", name), "create an object first, or make the method `static`");
        } else if implicit && self.is_static {
            self.err_hint(span, format!("cannot call the instance method `{}` from a static method", name), "make the method `static`, or call it on an object");
        }
        ret
    }

    fn call_variant(&mut self, enum_id: TypeId, idx: usize, args: &[Expr], span: Span, exp: Option<&Ty>) -> Ty {
        let prog = self.prog;
        let def = &prog.types[enum_id];
        let v = &def.variants[idx];
        let targs: Vec<Ty> = def.generics.iter().map(|_| self.fresh()).collect();
        let enum_ty = Ty::Named { id: enum_id, args: targs.clone() };
        if let Some(e) = exp {
            let r = self.shallow(e);
            if matches!(&r, Ty::Named { id, .. } if *id == enum_id) {
                let snap = self.infer.clone();
                if !self.unify(&enum_ty, &r) {
                    self.infer = snap;
                }
            }
        }
        let map = prog.param_map(def, &targs);
        if args.len() != v.fields.len() {
            let msg = if v.fields.is_empty() {
                format!("the variant `{}` takes no values", v.name)
            } else {
                format!("the variant `{}` takes {} value(s) but {} were supplied", v.name, v.fields.len(), args.len())
            };
            self.err(span, msg);
            for a in args {
                self.expr(a, None);
            }
            return enum_ty;
        }
        for (a, f) in args.iter().zip(v.fields.iter()) {
            let pt = subst(&f.ty, &map);
            let t = self.expr(a, Some(&pt));
            self.coerce_at(a.span, &t, &pt);
        }
        enum_ty
    }

    fn new_expr(&mut self, tr: &TypeRef, args: &[Expr], span: Span) -> Ty {
        let prog = self.prog;
        let ty = self.resolve_ty(tr);
        let Ty::Named { id, args: targs } = &ty else {
            if !matches!(ty, Ty::Error) {
                let msg = format!("cannot create an instance of `{}` with `new`", prog.show(&ty));
                self.err(span, msg);
            }
            for a in args {
                self.expr(a, None);
            }
            return Ty::Error;
        };
        let def = &prog.types[*id];
        let mut bad = None;
        if def.kind == DefKind::Interface {
            bad = Some(format!("cannot create an instance of the interface `{}`", def.name));
        } else if def.kind == DefKind::Enum {
            bad = Some(format!("`{}` is an enum; create a value with `{}.Variant`", def.name, def.name));
        } else if def.is_static {
            bad = Some(format!("cannot create an instance of the static class `{}`", def.name));
        } else if def.is_abstract {
            bad = Some(format!("cannot create an instance of the abstract class `{}`", def.name));
        }
        if let Some(m) = bad {
            self.err(span, m);
            for a in args {
                self.expr(a, None);
            }
            return ty.clone();
        }
        // `new shared<T>(args)` builds the T and wraps it
        let ctor_ty = if *id == prog.lang.shared { targs[0].clone() } else { ty.clone() };
        let Ty::Named { id: cid, args: cargs } = &ctor_ty else {
            for a in args {
                self.expr(a, None);
            }
            return ty.clone();
        };
        let cd = &prog.types[*cid];
        let map = prog.param_map(cd, cargs);
        let cands: Vec<Cand> = cd
            .ctors
            .iter()
            .map(|c| Cand {
                params: c.params.iter().map(|p| (subst(&p.ty, &map), p.has_default)).collect(),
                generics: Vec::new(),
                ret: ctor_ty.clone(),
                is_static: false,
                vis: c.vis,
                owner: *cid,
            })
            .collect();
        if cands.is_empty() {
            self.err(span, format!("`{}` has no constructor", cd.name));
            for a in args {
                self.expr(a, None);
            }
            return ty.clone();
        }
        if let Some((_, i)) = self.resolve_call(&cd.name, &cands, args, span, None) {
            let (vis, owner) = (cands[i].vis, cands[i].owner);
            self.check_access(vis, owner, span, &format!("the constructor of `{}`", cd.name));
        }
        ty.clone()
    }

    // ----------------------------------------------------------- operators

    fn unary(&mut self, op: UnOp, x: &Expr, exp: Option<&Ty>) -> Ty {
        let t = self.expr(x, exp);
        let r = self.peel(&t);
        match op {
            UnOp::Not => match &r {
                Ty::Prim(Prim::Bool) => r,
                Ty::Error | Ty::Never => Ty::Prim(Prim::Bool),
                other => {
                    let msg = format!("`!` needs a `bool`, found `{}`", self.prog.show(other));
                    self.err(x.span, msg);
                    Ty::Error
                }
            },
            UnOp::Neg => match &r {
                Ty::Prim(p) if p.is_numeric() && p.is_signed() => r.clone(),
                Ty::Prim(Prim::Byte | Prim::UShort) => Ty::Prim(Prim::Int),
                Ty::Prim(Prim::UInt) => Ty::Prim(Prim::Long),
                Ty::Error | Ty::Never => Ty::Error,
                other => {
                    let msg = format!("cannot negate a value of type `{}`", self.prog.show(other));
                    self.err(x.span, msg);
                    Ty::Error
                }
            },
        }
    }

    fn require_bool(&mut self, span: Span, t: &Ty) {
        let r = self.peel(t);
        if !matches!(r, Ty::Prim(Prim::Bool) | Ty::Error | Ty::Never) {
            self.mismatch(span, &Ty::Prim(Prim::Bool), &r);
        }
    }

    fn binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, span: Span) -> Ty {
        match op {
            BinOp::Coalesce => {
                let lt = self.expr(lhs, None);
                let lr = self.peel(&lt);
                let Some(inner) = self.prog.is_option(&lr) else {
                    if !lr.has_error() {
                        let msg = format!("the left side of `??` must be optional (`T?`), found `{}`", self.prog.show(&lr));
                        self.err(lhs.span, msg);
                    }
                    self.expr(rhs, None);
                    return Ty::Error;
                };
                let rt = self.expr(rhs, Some(&inner));
                self.coerce_at(rhs.span, &rt, &inner);
                inner
            }
            BinOp::And | BinOp::Or => {
                let a = self.expr(lhs, Some(&Ty::Prim(Prim::Bool)));
                let b = self.expr(rhs, Some(&Ty::Prim(Prim::Bool)));
                self.require_bool(lhs.span, &a);
                self.require_bool(rhs.span, &b);
                Ty::Prim(Prim::Bool)
            }
            _ => {
                let lt = self.expr(lhs, None);
                let lr = self.peel(&lt);
                let hint = if matches!(&lr, Ty::Prim(p) if p.is_numeric()) { Some(lr.clone()) } else { None };
                let rt = self.expr(rhs, hint.as_ref());
                let rr = self.peel(&rt);
                self.binary_types(op, &lr, &rr, span)
            }
        }
    }

    fn binary_types(&mut self, op: BinOp, l: &Ty, r: &Ty, span: Span) -> Ty {
        let is_cmp = matches!(op, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge);
        if l.has_error() || r.has_error() || matches!(l, Ty::Never) || matches!(r, Ty::Never) {
            return if is_cmp { Ty::Prim(Prim::Bool) } else { Ty::Error };
        }
        if let (Ty::Infer(_), _) | (_, Ty::Infer(_)) = (l, r) {
            self.unify(l, r);
            return if is_cmp { Ty::Prim(Prim::Bool) } else { self.resolve(l) };
        }
        let prog = self.prog;
        let bad = |this: &mut Self, what: &str| {
            let msg = format!("cannot {} `{}` and `{}`", what, prog.show(l), prog.show(r));
            this.err(span, msg);
            if is_cmp { Ty::Prim(Prim::Bool) } else { Ty::Error }
        };
        match op {
            BinOp::Add if matches!(l, Ty::Prim(Prim::String)) || matches!(r, Ty::Prim(Prim::String)) => {
                if matches!(l, Ty::Void) || matches!(r, Ty::Void) {
                    return bad(self, "add");
                }
                Ty::Prim(Prim::String)
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                if let (Ty::Prim(a), Ty::Prim(b)) = (l, r) {
                    if let Some(p) = Prim::promote(*a, *b) {
                        return Ty::Prim(p);
                    }
                }
                let word = match op {
                    BinOp::Add => "add",
                    BinOp::Sub => "subtract",
                    BinOp::Mul => "multiply",
                    BinOp::Div => "divide",
                    _ => "take the remainder of",
                };
                bad(self, word)
            }
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => match (l, r) {
                (Ty::Prim(a), Ty::Prim(b)) if a.is_numeric() && b.is_numeric() => Ty::Prim(Prim::Bool),
                (Ty::Prim(Prim::Char), Ty::Prim(Prim::Char)) | (Ty::Prim(Prim::String), Ty::Prim(Prim::String)) => Ty::Prim(Prim::Bool),
                _ => bad(self, "compare"),
            },
            BinOp::Eq | BinOp::Ne => {
                let snap = self.infer.clone();
                if self.coerce(l, r) || self.coerce(r, l) {
                    Ty::Prim(Prim::Bool)
                } else {
                    self.infer = snap;
                    bad(self, "compare")
                }
            }
            BinOp::And | BinOp::Or | BinOp::Coalesce => Ty::Error,
        }
    }

    // ------------------------------------------------ mutation and borrowing

    fn require_mutable(&mut self, v: &Val, span: Span, what: MutKind) {
        if v.ro {
            self.err_hint(
                span,
                format!("cannot modify `{}` through an immutable reference", v.name),
                "take a `mut ref` instead of a `ref`",
            );
            return;
        }
        let verb = match what {
            MutKind::Assign => "assign to",
            MutKind::Update => "modify",
            MutKind::Borrow => "borrow as mutable",
        };
        match &v.kind {
            ValKind::Local { kind, deferred } => {
                let ty_is_mut_ref = matches!(self.resolve(&v.ty), Ty::Ref { mutable: true, .. });
                match (what, kind) {
                    (MutKind::Borrow, LocalKind::Var) => {}
                    (MutKind::Borrow, _) if ty_is_mut_ref => {}
                    (MutKind::Assign, LocalKind::Var) | (MutKind::Update, LocalKind::Var) => {}
                    (MutKind::Assign, LocalKind::Let) if *deferred => {}
                    (_, LocalKind::Param) => self.err_hint(
                        span,
                        format!("cannot {} the parameter `{}`", verb, v.name),
                        "parameters are immutable; copy it into a `var` first, or take a `mut ref` parameter",
                    ),
                    _ => self.err_hint(
                        span,
                        format!("cannot {} `{}` because it is declared with `let`", verb, v.name),
                        "declare it with `var` to allow changes",
                    ),
                }
            }
            ValKind::Field { readonly } => {
                if *readonly && !self.in_ctor && what != MutKind::Borrow {
                    self.err(span, format!("cannot {} the readonly field `{}` outside a constructor", verb, v.name));
                    return;
                }
                self.require_root(v, span, verb);
            }
            ValKind::Prop { has_set, set_vis, owner } => {
                if what != MutKind::Borrow {
                    if !*has_set && !self.in_ctor {
                        self.err_hint(
                            span,
                            format!("the property `{}` is read-only", v.name),
                            "add a `set` accessor to allow assignment",
                        );
                        return;
                    }
                    if *has_set {
                        self.check_access(*set_vis, *owner, span, &format!("the setter of `{}`", v.name));
                    }
                }
                self.require_root(v, span, verb);
            }
            ValKind::Elem => self.require_root(v, span, verb),
            ValKind::Other => {}
        }
    }

    fn require_root(&mut self, v: &Val, span: Span, verb: &str) {
        if let Some(r) = &v.root {
            if !r.declared_mut {
                self.err_hint(
                    span,
                    format!("cannot {} `{}` because `{}` is not declared with `var`", verb, v.name, r.name),
                    format!("declare `{}` with `var`, or pass it as a `mut ref`", r.name),
                );
            }
        }
    }

    fn target_val(&mut self, target: &Expr) -> Option<Val> {
        match self.classify(target) {
            Res::Val(v) => Some(v),
            Res::Error => None,
            other => {
                self.res_value(other, target.span);
                None
            }
        }
    }

    fn assign(&mut self, op: AssignOp, target: &Expr, value: &Expr, span: Span) -> Ty {
        let plain = if let (AssignOp::Assign, ExprKind::Ident(n)) = (op, &target.kind) { self.find_local(n) } else { None };
        self.skip_moved = plain.is_some();
        let tv = self.target_val(target);
        self.skip_moved = false;
        if let Some(root) = root_ident(target) {
            if let Some((id, _)) = self.find_local(root) {
                if let Some(l) = self.live_loan(id, target.span, true).filter(|l| !l.mutable) {
                    let msg = format!("cannot assign to `{}` because it is borrowed by `{}`", root, l.holder);
                    self.err_hint(target.span, msg, format!("`{}` is still used later; finish using it first, or end the borrow in a smaller `{{ }}` block", l.holder));
                }
            }
        }
        let Some(v) = tv else {
            self.expr(value, None);
            return Ty::Void;
        };
        let tty = v.ty.clone();
        let kind = if op == AssignOp::Assign { MutKind::Assign } else { MutKind::Update };
        self.require_mutable(&v, target.span, kind);
        match op {
            AssignOp::Assign => {
                let vt = self.expr(value, Some(&tty));
                self.coerce_at(value.span, &vt, &tty);
                self.consume(value);
                if let Some((id, _)) = plain {
                    self.moved.remove(&id);
                }
            }
            _ => {
                let lr = self.peel(&tty);
                let vt = self.expr(value, Some(&lr));
                let rr = self.peel(&vt);
                let bop = match op {
                    AssignOp::Add => BinOp::Add,
                    AssignOp::Sub => BinOp::Sub,
                    AssignOp::Mul => BinOp::Mul,
                    AssignOp::Div => BinOp::Div,
                    _ => BinOp::Rem,
                };
                let res = self.binary_types(bop, &lr, &rr, span);
                self.coerce_at(span, &res, &lr);
            }
        }
        Ty::Void
    }

    fn postfix(&mut self, x: &Expr) -> Ty {
        let Some(v) = self.target_val(x) else { return Ty::Error };
        self.require_mutable(&v, x.span, MutKind::Update);
        let r = self.peel(&v.ty);
        match &r {
            Ty::Prim(p) if p.is_numeric() => r,
            Ty::Error => Ty::Error,
            other => {
                let msg = format!("`++` and `--` need a number, found `{}`", self.prog.show(other));
                self.err(x.span, msg);
                Ty::Error
            }
        }
    }

    fn borrow(&mut self, mutable: bool, x: &Expr) -> Ty {
        let v = match &x.kind {
            ExprKind::Ident(_) | ExprKind::Member { .. } | ExprKind::Index { .. } | ExprKind::This => match self.classify(x) {
                Res::Val(v) => v,
                Res::Error => return Ty::Error,
                other => {
                    self.res_value(other, x.span);
                    return Ty::Error;
                }
            },
            _ => {
                let t = self.expr(x, None);
                Val::plain(t)
            }
        };
        if mutable {
            self.require_mutable(&v, x.span, MutKind::Borrow);
        }
        if let ExprKind::Ident(n) = &x.kind {
            if let Some((id, _)) = self.find_local(n) {
                if mutable {
                    if let Some(l) = self.live_loan(id, x.span, true) {
                        if !l.mutable {
                            let msg = format!("cannot borrow `{}` as mutable because it is already borrowed by `{}`", n, l.holder);
                            self.err_hint(x.span, msg, "only one `mut ref` OR any number of `ref` can exist at the same time");
                        }
                    }
                }
            }
        }
        let inner = self.resolve(&v.ty);
        if let Ty::Ref { mutable: m0, inner: i0 } = &inner {
            if mutable && !*m0 {
                self.err_hint(x.span, "cannot borrow as mutable through an immutable reference", "the original reference is `ref`, not `mut ref`");
            }
            return Ty::Ref { mutable, inner: i0.clone() };
        }
        if matches!(inner, Ty::Void) {
            self.err(x.span, "cannot borrow a `void` value");
            return Ty::Error;
        }
        Ty::Ref { mutable, inner: Box::new(inner) }
    }

    // --------------------------------------------------- Result / Option / await

    fn try_expr(&mut self, x: &Expr, span: Span) -> Ty {
        let prog = self.prog;
        let t = self.expr(x, None);
        let r = self.peel(&t);
        let ret = self.ret.last().map(|c| c.ret.clone()).unwrap_or(Ty::Void);
        let fr = self.resolve(&ret);
        match &r {
            Ty::Named { id, args } if *id == prog.lang.option => {
                if prog.is_option(&fr).is_none() && !fr.has_error() {
                    self.err_hint(
                        span,
                        "`?` on an optional value can only be used in a method that returns an optional (`T?`)",
                        format!("this method returns `{}`", prog.show(&fr)),
                    );
                }
                args[0].clone()
            }
            Ty::Named { id, args } if *id == prog.lang.result => {
                match &fr {
                    Ty::Named { id: rid, args: rargs } if *rid == prog.lang.result => {
                        let (from, to) = (args[1].clone(), rargs[1].clone());
                        if !self.coerce(&from, &to) {
                            let msg = format!(
                                "the error type `{}` cannot be converted to this method's error type `{}`",
                                self.show(&from),
                                self.show(&to)
                            );
                            self.err(span, msg);
                        }
                    }
                    Ty::Error => {}
                    other => {
                        let hint = format!("this method returns `{}`", prog.show(other));
                        self.err_hint(span, "`?` on a `Result` can only be used in a method that returns a `Result`", hint);
                    }
                }
                args[0].clone()
            }
            Ty::Error | Ty::Never => Ty::Error,
            other => {
                let msg = format!("the `?` operator needs a `Result` or an optional value, found `{}`", prog.show(other));
                self.err(span, msg);
                Ty::Error
            }
        }
    }

    fn await_expr(&mut self, x: &Expr, span: Span) -> Ty {
        if !self.is_async {
            self.err_hint(span, "`await` can only be used inside an `async` method", "add `async` to the method and return `Task<T>`");
        }
        let t = self.expr(x, None);
        let r = self.peel(&t);
        match &r {
            Ty::Named { id, args } if *id == self.prog.lang.task && args.len() == 1 => args[0].clone(),
            Ty::Error | Ty::Never => Ty::Error,
            other => {
                let msg = format!("`await` needs a `Task<T>`, found `{}`", self.prog.show(other));
                self.err(x.span, msg);
                Ty::Error
            }
        }
    }

    // -------------------------------------------------------------- match

    fn enum_variant_in(&self, ty: &Ty, name: &str) -> Option<(TypeId, usize)> {
        if let Ty::Named { id, .. } = ty {
            let d = &self.prog.types[*id];
            if d.kind == DefKind::Enum {
                if let Some(i) = d.variants.iter().position(|v| v.name == name) {
                    return Some((*id, i));
                }
            }
        }
        None
    }

    fn variant_for(&mut self, path: &[String], sty: &Ty, span: Span) -> Option<(TypeId, usize)> {
        let prog = self.prog;
        if path.len() == 1 {
            let name = &path[0];
            if let Some(r) = self.enum_variant_in(sty, name) {
                return Some(r);
            }
            if let Some(&r) = prog.prelude_variants.get(name) {
                return Some(r);
            }
            let msg = format!("`{}` is not a variant of `{}`", name, prog.show(sty));
            self.err(span, msg);
            return None;
        }
        let (vname, tpath) = (path.last().unwrap(), &path[..path.len() - 1]);
        let scope = prog.types[self.tid].scope.clone();
        let id = {
            let ctx = ResolveCtx { scope: &scope, owner: Some(self.tid), tparams: &self.tparams };
            prog.lookup_type(tpath, &ctx)
        };
        let Some(id) = id else {
            self.err(span, format!("cannot find the type `{}`", tpath.join(".")));
            return None;
        };
        let d = &prog.types[id];
        match d.variants.iter().position(|v| &v.name == vname) {
            Some(i) if d.kind == DefKind::Enum => Some((id, i)),
            _ => {
                self.err(span, format!("`{}` has no variant `{}`", d.name, vname));
                None
            }
        }
    }

    fn pattern(&mut self, p: &Pattern, ty: &Ty) -> PatCover {
        let sty = self.peel(ty);
        match &p.kind {
            PatternKind::Wildcard => PatCover::All,
            PatternKind::Literal(e) => {
                let t = self.expr(e, Some(&sty));
                if !self.coerce(&t, &sty) {
                    let (e1, f1) = (self.show(&sty), self.show(&t));
                    if !sty.has_error() && !t.has_error() {
                        self.err(p.span, format!("this pattern has type `{}` but the matched value has type `{}`", f1, e1));
                    }
                    return PatCover::Nothing;
                }
                match &e.kind {
                    ExprKind::Bool(b) => PatCover::Bool(*b),
                    _ => PatCover::Nothing,
                }
            }
            PatternKind::Binding(name) => {
                if let Some((eid, idx)) = self.enum_variant_in(&sty, name) {
                    let n = self.prog.types[eid].variants[idx].fields.len();
                    if n > 0 {
                        self.err_hint(p.span, format!("the variant `{}` carries {} value(s)", name, n), format!("write `{}(...)`", name));
                    }
                    return PatCover::Variant(idx);
                }
                if let Some(&(eid, idx)) = self.prog.prelude_variants.get(name) {
                    // `None` where the scrutinee is optional
                    if self.prog.is_option(&sty).is_some() && self.prog.types[eid].variants[idx].fields.is_empty() {
                        return PatCover::Variant(idx);
                    }
                }
                self.declare(name, sty.clone(), LocalKind::Let, false, p.span);
                PatCover::All
            }
            PatternKind::Path(path) => {
                let Some((eid, idx)) = self.variant_for(path, &sty, p.span) else { return PatCover::Nothing };
                self.match_variant_type(eid, &sty, p.span);
                let n = self.prog.types[eid].variants[idx].fields.len();
                if n > 0 {
                    let vname = self.prog.types[eid].variants[idx].name.clone();
                    self.err_hint(p.span, format!("the variant `{}` carries {} value(s)", vname, n), format!("write `{}(...)`", vname));
                }
                PatCover::Variant(idx)
            }
            PatternKind::Variant { path, args } => {
                let Some((eid, idx)) = self.variant_for(path, &sty, p.span) else {
                    for a in args {
                        self.pattern(a, &Ty::Error);
                    }
                    return PatCover::Nothing;
                };
                self.match_variant_type(eid, &sty, p.span);
                let prog = self.prog;
                let def = &prog.types[eid];
                let v = &def.variants[idx];
                let rs = self.resolve(&sty);
                let eargs = match &rs {
                    Ty::Named { id, args } if *id == eid => args.clone(),
                    _ => def.generics.iter().map(|_| Ty::Error).collect(),
                };
                let map = prog.param_map(def, &eargs);
                if args.len() != v.fields.len() {
                    self.err(p.span, format!("the variant `{}` carries {} value(s) but the pattern has {}", v.name, v.fields.len(), args.len()));
                    for a in args {
                        self.pattern(a, &Ty::Error);
                    }
                    return PatCover::Nothing;
                }
                let mut irrefutable = true;
                for (a, f) in args.iter().zip(v.fields.iter()) {
                    let ft = subst(&f.ty, &map);
                    if !matches!(self.pattern(a, &ft), PatCover::All) {
                        irrefutable = false;
                    }
                }
                if irrefutable {
                    PatCover::Variant(idx)
                } else {
                    PatCover::Nothing
                }
            }
        }
    }

    /// Make sure the matched value really is of the enum the pattern names.
    fn match_variant_type(&mut self, eid: TypeId, sty: &Ty, span: Span) {
        let prog = self.prog;
        let def = &prog.types[eid];
        let args: Vec<Ty> = def.generics.iter().map(|_| self.fresh()).collect();
        let want = Ty::Named { id: eid, args };
        if matches!(self.resolve(sty), Ty::Error) {
            return;
        }
        if !self.unify(&want, sty) {
            let msg = format!("this pattern is a variant of `{}`, but the matched value has type `{}`", def.name, self.show(sty));
            self.err(span, msg);
        }
    }

    fn match_expr(&mut self, scrutinee: &Expr, arms: &[MatchArm], span: Span, exp: Option<&Ty>) -> Ty {
        let st = self.expr(scrutinee, None);
        let sty = self.peel(&st);
        let mut result: Option<Ty> = None;
        let mut covers: Vec<(PatCover, bool)> = Vec::new();
        let before = self.moved.clone();
        let mut states: Vec<HashMap<usize, Span>> = Vec::new();
        for arm in arms {
            self.moved = before.clone();
            self.push_scope();
            let cover = self.pattern(&arm.pattern, &sty);
            if let Some(g) = &arm.guard {
                self.cond(g);
            }
            let hint = result.clone().or_else(|| exp.cloned());
            let bt = self.expr(&arm.body, hint.as_ref());
            self.pop_scope();
            covers.push((cover, arm.guard.is_some()));
            if matches!(self.shallow(&bt), Ty::Never) || expr_diverges(&arm.body) {
                continue;
            }
            states.push(self.moved.clone());
            match &result {
                None => result = Some(bt),
                Some(r) => {
                    let r = r.clone();
                    let either_void = matches!(self.shallow(&r), Ty::Void) || matches!(self.shallow(&bt), Ty::Void);
                    if either_void {
                        result = Some(Ty::Void);
                    } else if !self.coerce(&bt, &r) {
                        let msg = format!(
                            "the arms of this `match` have different types: this one is `{}`, earlier ones are `{}`",
                            self.show(&bt),
                            self.show(&r)
                        );
                        self.err(arm.body.span, msg);
                    }
                }
            }
        }
        self.merge_states(before, states);
        // exhaustiveness
        let rs = self.resolve(&sty);
        if !rs.has_error() {
            let catch_all = covers.iter().any(|(c, guarded)| !*guarded && matches!(c, PatCover::All));
            if !catch_all {
                let prog = self.prog;
                match &rs {
                    Ty::Named { id, .. } if prog.types[*id].kind == DefKind::Enum => {
                        let covered: HashSet<usize> = covers
                            .iter()
                            .filter(|(_, g)| !*g)
                            .filter_map(|(c, _)| if let PatCover::Variant(i) = c { Some(*i) } else { None })
                            .collect();
                        let missing: Vec<String> = prog.types[*id]
                            .variants
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| !covered.contains(i))
                            .map(|(_, v)| format!("`{}`", v.name))
                            .collect();
                        if !missing.is_empty() {
                            self.err_hint(
                                span,
                                format!("this `match` does not cover every case: missing {}", missing.join(", ")),
                                "add the missing arms, or a catch-all `_ => ...` arm",
                            );
                        }
                    }
                    Ty::Prim(Prim::Bool) => {
                        let has = |b: bool| covers.iter().any(|(c, g)| !*g && matches!(c, PatCover::Bool(x) if *x == b));
                        if !(has(true) && has(false)) {
                            self.err_hint(span, "this `match` does not cover both `true` and `false`", "add the missing arm, or a catch-all `_ => ...` arm");
                        }
                    }
                    _ => {
                        self.err_hint(span, "this `match` does not cover every possible value", "add a catch-all `_ => ...` arm");
                    }
                }
            }
        }
        match result {
            Some(r) => r,
            None => {
                if arms.is_empty() {
                    Ty::Void
                } else {
                    Ty::Never
                }
            }
        }
    }
}
