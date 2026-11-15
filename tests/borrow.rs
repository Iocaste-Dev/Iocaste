use iocaste::{parser, sema};

const HEAD: &str = "namespace T;\nimport Ioc.Console;\nimport Ioc.Collections;\n\
class Player { public name: string; public score: int = 0; public constructor(n: string) { this.name = n; } }\n\
class G {\n\
  static show(p: ref Player): void { }\n\
  static bump(p: mut ref Player): void { }\n\
  static take(p: Player): void { }\n\
  static two(a: mut ref Player, b: ref Player): void { }\n\
  static cond(): bool { return true; }\n";

fn errors(body: &str) -> Vec<String> {
    let src = format!("{}{}\n}}", HEAD, body);
    let ast = parser::parse(&src).expect("test source must parse");
    sema::analyze(&[&ast]).into_iter().map(|e| e.diag.message).collect()
}

fn main_body(stmts: &str) -> String {
    format!("static main(args: string[]): int {{ {} return 0; }}", stmts)
}

fn ok(stmts: &str) {
    let e = errors(&main_body(stmts));
    assert!(e.is_empty(), "expected no errors, got: {:#?}", e);
}

fn fails(stmts: &str, needle: &str) {
    let e = errors(&main_body(stmts));
    assert!(e.iter().any(|m| m.contains(needle)), "expected `{}`, got: {:#?}", needle, e);
}

#[test]
fn use_after_move() {
    fails("let p = new Player(\"a\"); take(p); show(ref p);", "use of moved value `p`");
}

#[test]
fn move_by_let() {
    fails("let p = new Player(\"a\"); let q = p; Console.println(p.name);", "use of moved value `p`");
}

#[test]
fn move_then_reassign_is_fine() {
    ok("var p = new Player(\"a\"); take(p); p = new Player(\"b\"); show(ref p);");
}

#[test]
fn copy_types_are_not_moved() {
    ok("let n = 5; let m = n; let k = n + m; let s = \"hi\"; let t = s; Console.println(s);");
}

#[test]
fn borrowing_does_not_move() {
    ok("var p = new Player(\"a\"); show(ref p); bump(mut ref p); show(ref p);");
}

#[test]
fn moved_in_one_branch() {
    fails("let p = new Player(\"a\"); if (cond()) { take(p); } show(ref p);", "use of moved value");
}

#[test]
fn moved_in_branch_that_returns_is_fine() {
    ok("let p = new Player(\"a\"); if (cond()) { take(p); return 1; } show(ref p);");
}

#[test]
fn moved_in_both_branches_then_used() {
    fails("let p = new Player(\"a\"); if (cond()) { take(p); } else { take(p); } show(ref p);", "use of moved value");
}

#[test]
fn moved_in_loop() {
    fails("let p = new Player(\"a\"); while (cond()) { take(p); }", "moved inside a loop");
}

#[test]
fn moved_in_loop_then_break_is_fine() {
    ok("let p = new Player(\"a\"); while (cond()) { take(p); break; }");
}

#[test]
fn new_value_each_iteration_is_fine() {
    ok("for (var i = 0; i < 3; i++) { let p = new Player(\"a\"); take(p); }");
}

#[test]
fn match_arm_moves() {
    fails(
        "let p = new Player(\"a\"); let n = 1; match (n) { 1 => take(p), _ => { } } show(ref p);",
        "use of moved value",
    );
}

#[test]
fn same_variable_twice_in_a_call() {
    fails("var p = new Player(\"a\"); two(mut ref p, ref p);", "used more than once");
    fails("let p = new Player(\"a\"); var q = new Player(\"b\"); take(p); two(mut ref q, ref q);", "used more than once");
}

#[test]
fn two_shared_borrows_are_fine() {
    ok("let p = new Player(\"a\"); show(ref p); show(ref p);");
}

#[test]
fn assign_while_borrowed() {
    fails(
        "var p = new Player(\"a\"); let r = ref p; p = new Player(\"b\"); show(r);",
        "cannot assign to `p` because it is borrowed",
    );
}

#[test]
fn field_write_while_borrowed() {
    fails(
        "var p = new Player(\"a\"); let r = ref p; p.score = 3; show(r);",
        "cannot assign to `p` because it is borrowed",
    );
}

#[test]
fn borrow_ended_is_fine() {
    ok("var p = new Player(\"a\"); let r = ref p; show(r); p.score = 3;");
}

#[test]
fn mutable_borrow_blocks_reads() {
    fails(
        "var p = new Player(\"a\"); let r = mut ref p; show(ref p); bump(r);",
        "while it is mutably borrowed",
    );
}

#[test]
fn mut_borrow_while_shared_borrowed() {
    fails(
        "var p = new Player(\"a\"); let r = ref p; bump(mut ref p); show(r);",
        "as mutable because it is already borrowed",
    );
}

#[test]
fn move_while_borrowed() {
    fails("let p = new Player(\"a\"); let r = ref p; take(p); show(r);", "because it is borrowed");
}

#[test]
fn return_reference_to_local() {
    let e = errors("static f(): ref Player { let p = new Player(\"a\"); return ref p; }");
    assert!(e.iter().any(|m| m.contains("reference to the local variable")), "{:#?}", e);
}

#[test]
fn array_and_new_arguments_move() {
    fails("let p = new Player(\"a\"); let l = new List<Player>(); l.add(p); show(ref p);", "use of moved value");
}
