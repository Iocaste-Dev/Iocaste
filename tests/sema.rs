use iocaste::{parser, sema};

fn errors(body: &str) -> Vec<String> {
    let src = format!(
        "namespace T;\nimport Ioc.Console;\nimport Ioc.Collections;\n{}",
        body
    );
    let ast = parser::parse(&src).expect("test source must parse");
    sema::analyze(&[&ast]).into_iter().map(|e| e.diag.message).collect()
}

fn ok(body: &str) {
    let e = errors(body);
    assert!(e.is_empty(), "expected no errors, got: {:#?}", e);
}

fn fails(body: &str, needle: &str) {
    let e = errors(body);
    assert!(
        e.iter().any(|m| m.contains(needle)),
        "expected an error containing `{}`, got: {:#?}",
        needle,
        e
    );
}

fn in_main(stmts: &str) -> String {
    format!("class P {{ static main(args: string[]): int {{ {} return 0; }} }}", stmts)
}

#[test]
fn prelude_is_clean() {
    ok("class P { }");
}

#[test]
fn examples_check() {
    for f in ["examples/hello.ioc", "examples/typed.ioc"] {
        let src = std::fs::read_to_string(f).unwrap();
        let ast = parser::parse(&src).unwrap();
        let e = sema::analyze(&[&ast]);
        assert!(e.is_empty(), "{}: {:#?}", f, e.iter().map(|x| &x.diag.message).collect::<Vec<_>>());
    }
}

#[test]
fn valid_basics() {
    ok(&in_main("let x = 1; var y = x + 2; y += 3; Console.println($\"{y}\");"));
    ok(&in_main("let l = new List<int>(); l.add(1); let n: int = l.count;"));
    ok(&in_main("let a: long = 1; let b: double = 2;"));
}

#[test]
fn undefined_variable() {
    fails(&in_main("let x = nope;"), "cannot find `nope`");
}

#[test]
fn wrong_argument_type() {
    fails(&in_main("Console.println(5);"), "");
}

#[test]
fn wrong_arity() {
    fails(
        "class P { static f(a: int): int { return a; } static g(): int { return f(1, 2); } }",
        "",
    );
}

#[test]
fn missing_member() {
    fails(&in_main("let s = \"hi\"; s.nothing();"), "nothing");
}

#[test]
fn assign_to_let() {
    fails(&in_main("let x = 1; x = 2;"), "");
}

#[test]
fn return_mismatch() {
    fails("class P { static f(): int { return \"no\"; } }", "");
}

#[test]
fn missing_return() {
    fails("class P { static f(): int { } }", "");
}

#[test]
fn non_exhaustive_match() {
    fails(
        "enum E { A, B } class P { static f(e: E): int { return match (e) { A => 1, }; } }",
        "",
    );
}

#[test]
fn unknown_type() {
    fails("class P { static f(x: Nope): void { } }", "Nope");
}

#[test]
fn unimplemented_interface() {
    fails("interface I { f(): int; } class C : I { }", "");
}

#[test]
fn this_in_static() {
    fails("class P { x: int; static f(): int { return this.x; } }", "");
}

#[test]
fn question_outside_result() {
    fails(
        "class P { static g(): Result<int, string> { return Ok(1); } static f(): int { let v = g()?; return v; } }",
        "",
    );
}

#[test]
fn mismatched_condition() {
    fails(&in_main("if (1) { }"), "");
}

#[test]
fn private_access() {
    fails(
        "class A { private x: int; } class P { static f(a: A): int { return a.x; } }",
        "",
    );
}

#[test]
fn generic_inference() {
    ok(&in_main("let l = new List<string>(); l.add(\"a\"); let s: string = l.get(0);"));
    fails(&in_main("let l = new List<string>(); l.add(1);"), "");
}
