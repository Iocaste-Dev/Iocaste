//! Pretty-printer for the AST (`ioc ast <file>`).

use crate::ast::*;

struct Node {
    label: String,
    kids: Vec<Node>,
}

fn n(label: impl Into<String>, kids: Vec<Node>) -> Node {
    Node { label: label.into(), kids }
}

fn leaf(label: impl Into<String>) -> Node {
    n(label, Vec::new())
}

/// Render the whole tree. With `ascii = true` only plain ASCII characters are used.
pub fn dump(file: &SourceFile, ascii: bool) -> String {
    let root = n("SourceFile", file.items.iter().map(item_n).collect());
    let mut out = String::new();
    render(&root, "", true, true, ascii, &mut out);
    out
}

fn render(node: &Node, prefix: &str, last: bool, root: bool, ascii: bool, out: &mut String) {
    let (tee, elbow, bar, gap) = if ascii { ("|- ", "`- ", "|  ", "   ") } else { ("├─ ", "└─ ", "│  ", "   ") };
    if !root {
        out.push_str(prefix);
        out.push_str(if last { elbow } else { tee });
    }
    out.push_str(&node.label);
    out.push('\n');
    let child_prefix = if root { String::new() } else { format!("{}{}", prefix, if last { gap } else { bar }) };
    for (i, k) in node.kids.iter().enumerate() {
        render(k, &child_prefix, i + 1 == node.kids.len(), false, ascii, out);
    }
}

// ---------------------------------------------------------------- helpers

fn fmt_path(p: &[String]) -> String {
    p.join(".")
}

fn fmt_mods(m: &Modifiers) -> String {
    if m.is_empty() {
        String::new()
    } else {
        format!(" [{}]", m.0.iter().map(|x| x.as_str()).collect::<Vec<_>>().join(" "))
    }
}

fn fmt_ty(t: &TypeRef) -> String {
    match &t.kind {
        TypeRefKind::Named { path, args } => {
            let mut s = path.join(".");
            if !args.is_empty() {
                s.push('<');
                s.push_str(&args.iter().map(fmt_ty).collect::<Vec<_>>().join(", "));
                s.push('>');
            }
            s
        }
        TypeRefKind::Array(inner) => format!("{}[]", fmt_ty(inner)),
        TypeRefKind::Optional(inner) => format!("{}?", fmt_ty(inner)),
        TypeRefKind::Ref { mutable, inner } => format!("{}ref {}", if *mutable { "mut " } else { "" }, fmt_ty(inner)),
    }
}

fn fmt_params(ps: &[Param]) -> String {
    ps.iter()
        .map(|p| {
            let mut s = format!("{}: {}", p.name, fmt_ty(&p.ty));
            if p.default.is_some() {
                s.push_str(" = ...");
            }
            s
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn fmt_generics(g: &[GenericParam]) -> String {
    if g.is_empty() {
        String::new()
    } else {
        format!("<{}>", g.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", "))
    }
}

fn attrs_n(a: &[Attribute]) -> Vec<Node> {
    a.iter()
        .map(|x| n(format!("Attribute @{}", fmt_path(&x.name)), x.args.iter().map(expr_n).collect()))
        .collect()
}

// ------------------------------------------------------------------ items

fn item_n(it: &Item) -> Node {
    match it {
        Item::Namespace(ns) => n(
            format!("Namespace {}{}", fmt_path(&ns.name), if ns.file_scoped { " (file-scoped)" } else { "" }),
            ns.items.iter().map(item_n).collect(),
        ),
        Item::Import(i) => leaf(format!("Import {}", fmt_path(&i.path))),
        Item::Type(t) => type_n(t),
        Item::Enum(e) => enum_n(e),
    }
}

fn type_n(t: &TypeDecl) -> Node {
    let kind = match t.kind {
        TypeKind::Class => "Class",
        TypeKind::Struct => "Struct",
        TypeKind::Record => "Record",
        TypeKind::Interface => "Interface",
    };
    let mut label = format!("{} {}{}{}", kind, t.name, fmt_generics(&t.generics), fmt_mods(&t.modifiers));
    if t.kind == TypeKind::Record {
        label.push_str(&format!(" ({})", fmt_params(&t.record_params)));
    }
    if !t.bases.is_empty() {
        label.push_str(" : ");
        label.push_str(&t.bases.iter().map(fmt_ty).collect::<Vec<_>>().join(", "));
    }
    let mut kids = attrs_n(&t.attributes);
    for g in &t.generics {
        if !g.constraints.is_empty() {
            kids.push(leaf(format!(
                "Where {} : {}",
                g.name,
                g.constraints.iter().map(fmt_ty).collect::<Vec<_>>().join(", ")
            )));
        }
    }
    kids.extend(t.members.iter().map(member_n));
    n(label, kids)
}

fn enum_n(e: &EnumDecl) -> Node {
    let mut kids = attrs_n(&e.attributes);
    for v in &e.variants {
        let fields = if v.fields.is_empty() { String::new() } else { format!("({})", fmt_params(&v.fields)) };
        let vk = v.value.iter().map(|x| n("value", vec![expr_n(x)])).collect();
        kids.push(n(format!("Variant {}{}", v.name, fields), vk));
    }
    n(format!("Enum {}{}{}", e.name, fmt_generics(&e.generics), fmt_mods(&e.modifiers)), kids)
}

fn body_kids(b: &Body) -> Vec<Node> {
    match b {
        Body::None => Vec::new(),
        Body::Expr(e) => vec![n("ExprBody", vec![expr_n(e)])],
        Body::Block(bl) => vec![block_n(bl)],
    }
}

fn member_n(m: &Member) -> Node {
    match m {
        Member::Field(f) => {
            let mut kids = attrs_n(&f.attributes);
            if let Some(i) = &f.init {
                kids.push(n("init", vec![expr_n(i)]));
            }
            n(format!("Field {}: {}{}", f.name, fmt_ty(&f.ty), fmt_mods(&f.modifiers)), kids)
        }
        Member::Property(p) => {
            let mut kids = attrs_n(&p.attributes);
            for a in &p.accessors {
                let kind = if a.kind == AccessorKind::Get { "Get" } else { "Set" };
                kids.push(n(format!("{}{}", kind, fmt_mods(&a.modifiers)), body_kids(&a.body)));
            }
            if let Some(i) = &p.init {
                kids.push(n("init", vec![expr_n(i)]));
            }
            n(format!("Property {}: {}{}", p.name, fmt_ty(&p.ty), fmt_mods(&p.modifiers)), kids)
        }
        Member::Method(me) => {
            let ret = me.ret.as_ref().map(|r| format!(": {}", fmt_ty(r))).unwrap_or_default();
            let mut kids = attrs_n(&me.attributes);
            for g in &me.generics {
                if !g.constraints.is_empty() {
                    kids.push(leaf(format!("Where {}", g.name)));
                }
            }
            kids.extend(body_kids(&me.body));
            n(
                format!("Method {}{}({}){}{}", me.name, fmt_generics(&me.generics), fmt_params(&me.params), ret, fmt_mods(&me.modifiers)),
                kids,
            )
        }
        Member::Constructor(c) => {
            let mut kids = attrs_n(&c.attributes);
            kids.push(block_n(&c.body));
            n(format!("Constructor ({}){}", fmt_params(&c.params), fmt_mods(&c.modifiers)), kids)
        }
        Member::Destructor(d) => {
            let mut kids = attrs_n(&d.attributes);
            kids.push(block_n(&d.body));
            n(format!("Destructor{}", fmt_mods(&d.modifiers)), kids)
        }
        Member::Type(t) => type_n(t),
        Member::Enum(e) => enum_n(e),
    }
}

// ------------------------------------------------------------- statements

fn block_n(b: &Block) -> Node {
    n("Block", b.stmts.iter().map(stmt_n).collect())
}

fn stmt_n(s: &Stmt) -> Node {
    match &s.kind {
        StmtKind::Local { mutable, name, ty, init } => {
            let mut label = format!("{} {}", if *mutable { "Var" } else { "Let" }, name);
            if let Some(t) = ty {
                label.push_str(&format!(": {}", fmt_ty(t)));
            }
            n(label, init.iter().map(expr_n).collect())
        }
        StmtKind::Expr(e) => expr_n(e),
        StmtKind::Block(b) => block_n(b),
        StmtKind::If { cond, then, otherwise } => {
            let mut kids = vec![n("cond", vec![expr_n(cond)]), n("then", vec![stmt_n(then)])];
            if let Some(o) = otherwise {
                kids.push(n("else", vec![stmt_n(o)]));
            }
            n("If", kids)
        }
        StmtKind::While { cond, body } => n("While", vec![n("cond", vec![expr_n(cond)]), n("body", vec![stmt_n(body)])]),
        StmtKind::For { init, cond, update, body } => {
            let mut kids = Vec::new();
            if let Some(i) = init {
                kids.push(n("init", vec![stmt_n(i)]));
            }
            if let Some(c) = cond {
                kids.push(n("cond", vec![expr_n(c)]));
            }
            if let Some(u) = update {
                kids.push(n("update", vec![expr_n(u)]));
            }
            kids.push(n("body", vec![stmt_n(body)]));
            n("For", kids)
        }
        StmtKind::ForIn { mutable, name, iter, body } => n(
            format!("ForIn {} {}", if *mutable { "var" } else { "let" }, name),
            vec![n("in", vec![expr_n(iter)]), n("body", vec![stmt_n(body)])],
        ),
        StmtKind::Return(v) => n("Return", v.iter().map(expr_n).collect()),
        StmtKind::Break => leaf("Break"),
        StmtKind::Continue => leaf("Continue"),
        StmtKind::Unsafe(b) => n("Unsafe", vec![block_n(b)]),
    }
}

// ------------------------------------------------------------ expressions

fn expr_n(e: &Expr) -> Node {
    match &e.kind {
        ExprKind::Int { text, suffix } => leaf(format!("Int {}{}", text, suffix)),
        ExprKind::Float { text, suffix } => leaf(format!("Float {}{}", text, suffix)),
        ExprKind::Str(s) => leaf(format!("Str {:?}", s)),
        ExprKind::Interp(segs) => n(
            "Interp",
            segs.iter()
                .map(|s| match s {
                    InterpSegment::Text(t) => leaf(format!("Text {:?}", t)),
                    InterpSegment::Expr(x) => n("Hole", vec![expr_n(x)]),
                })
                .collect(),
        ),
        ExprKind::Char(c) => leaf(format!("Char {:?}", c)),
        ExprKind::Bool(b) => leaf(format!("Bool {}", b)),
        ExprKind::Ident(s) => leaf(format!("Ident {}", s)),
        ExprKind::This => leaf("This"),
        ExprKind::Base => leaf("Base"),
        ExprKind::Member { target, name } => n(format!("Member .{}", name), vec![expr_n(target)]),
        ExprKind::Index { target, index } => n("Index", vec![expr_n(target), expr_n(index)]),
        ExprKind::Call { callee, args } => {
            let mut kids = vec![expr_n(callee)];
            kids.extend(args.iter().map(expr_n));
            n("Call", kids)
        }
        ExprKind::New { ty, args } => n(format!("New {}", fmt_ty(ty)), args.iter().map(expr_n).collect()),
        ExprKind::Unary { op, expr } => {
            n(format!("Unary {}", if *op == UnOp::Neg { "-" } else { "!" }), vec![expr_n(expr)])
        }
        ExprKind::Binary { op, lhs, rhs } => n(format!("Binary {}", op.as_str()), vec![expr_n(lhs), expr_n(rhs)]),
        ExprKind::Assign { op, target, value } => {
            n(format!("Assign {}", op.as_str()), vec![expr_n(target), expr_n(value)])
        }
        ExprKind::Postfix { op, expr } => {
            n(format!("Postfix {}", if *op == PostOp::Inc { "++" } else { "--" }), vec![expr_n(expr)])
        }
        ExprKind::Try(x) => n("Try ?", vec![expr_n(x)]),
        ExprKind::Await(x) => n("Await", vec![expr_n(x)]),
        ExprKind::Borrow { mutable, expr } => {
            n(if *mutable { "Borrow mut ref" } else { "Borrow ref" }, vec![expr_n(expr)])
        }
        ExprKind::Match { scrutinee, arms } => {
            let mut kids = vec![n("scrutinee", vec![expr_n(scrutinee)])];
            for a in arms {
                let mut ak = Vec::new();
                if let Some(g) = &a.guard {
                    ak.push(n("guard", vec![expr_n(g)]));
                }
                ak.push(expr_n(&a.body));
                kids.push(n(format!("Arm {} =>", fmt_pat(&a.pattern)), ak));
            }
            n("Match", kids)
        }
        ExprKind::Spawn(b) => n("Spawn", vec![block_n(b)]),
        ExprKind::Block(b) => block_n(b),
        ExprKind::Return(v) => n("Return", v.iter().map(|x| expr_n(x)).collect()),
        ExprKind::Array(items) => n("Array", items.iter().map(expr_n).collect()),
    }
}

fn fmt_pat(p: &Pattern) -> String {
    match &p.kind {
        PatternKind::Wildcard => "_".into(),
        PatternKind::Literal(e) => fmt_lit(e),
        PatternKind::Binding(s) => s.clone(),
        PatternKind::Path(p) => fmt_path(p),
        PatternKind::Variant { path, args } => {
            format!("{}({})", fmt_path(path), args.iter().map(fmt_pat).collect::<Vec<_>>().join(", "))
        }
    }
}

fn fmt_lit(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Int { text, suffix } | ExprKind::Float { text, suffix } => format!("{}{}", text, suffix),
        ExprKind::Str(s) => format!("{:?}", s),
        ExprKind::Char(c) => format!("{:?}", c),
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::Unary { expr, .. } => format!("-{}", fmt_lit(expr)),
        _ => "?".into(),
    }
}
