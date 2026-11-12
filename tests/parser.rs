use iocaste::parser::parse;

fn ok(src: &str) {
    if let Err(d) = parse(src) {
        panic!("{}", d.render("<test>", src));
    }
}

fn err(src: &str) -> iocaste::diag::Diagnostic {
    match parse(src) {
        Ok(_) => panic!("expected a syntax error for:\n{}", src),
        Err(d) => d,
    }
}

/// Wrap statements in a method so they can be parsed.
fn in_method(body: &str) -> String {
    format!("class A {{ run(): void {{ {} }} }}", body)
}

#[test]
fn hello_example_parses() {
    ok(&std::fs::read_to_string("examples/hello.ioc").unwrap());
}

#[test]
fn features_example_parses() {
    ok(&std::fs::read_to_string("examples/features.ioc").unwrap());
}

#[test]
fn empty_file_is_fine() {
    ok("");
    ok("// only a comment\n");
}

#[test]
fn file_scoped_and_block_namespaces() {
    ok("namespace A.B; class C {}");
    ok("namespace A { class C {} namespace B { class D {} } }");
}

#[test]
fn precedence_is_correct() {
    // `a + b * c` must group as a + (b * c): the dump shows `*` nested under `+`
    let ast = parse(&in_method("let x = a + b * c;")).unwrap();
    let text = iocaste::dump::dump(&ast, true);
    let plus = text.find("Binary +").unwrap();
    let star = text.find("Binary *").unwrap();
    assert!(plus < star, "{}", text);
}

#[test]
fn return_inside_coalesce_parses() {
    ok(&in_method("let v = int.tryParse(t) ?? return Err(Bad);"));
}

#[test]
fn match_statement_needs_no_semicolon() {
    ok(&in_method("match (x) { _ => f(), } g();"));
}

#[test]
fn nested_generics_close_correctly() {
    ok("class A { x: List<List<int>>; y: Dictionary<string, List<int>?>; }");
}

// ------------------------------------------------------------------ errors

#[test]
fn missing_semicolon() {
    let d = err("class A {\n    x: int\n}");
    assert!(d.message.contains("expected `;`"), "{}", d.message);
    // reported at the end of the line that is missing it, not at the `}` on the next line
    assert_eq!((d.span.line, d.span.col), (2, 11));
}

#[test]
fn csharp_style_member_gets_a_hint() {
    let d = err("class A { int x; }");
    assert!(d.hint.unwrap().contains("type comes after the name"));
}

#[test]
fn no_null() {
    let d = err(&in_method("let x = null;"));
    assert!(d.message.contains("no `null`"));
}

#[test]
fn no_exceptions() {
    assert!(err(&in_method("try { }")).message.contains("exceptions"));
    assert!(err(&in_method("throw e;")).message.contains("exceptions"));
}

#[test]
fn no_top_level_functions() {
    assert!(err("func main() {}").message.contains("top level"));
}

#[test]
fn unclosed_block_points_at_the_opening_brace() {
    let d = err("class A {\n  run(): void {\n    let x = 1;\n}\n");
    assert_eq!(d.span.line, 1); // the class body that is never closed
}

#[test]
fn reserved_word_as_name() {
    assert!(err("class A { class: int; }").message.contains("expected"));
    assert!(err(&in_method("let match = 1;")).message.contains("reserved"));
}

#[test]
fn invalid_assignment_target() {
    assert!(err(&in_method("f() = 1;")).message.contains("invalid assignment target"));
}

#[test]
fn let_without_type_or_value() {
    assert!(err(&in_method("let x;")).message.contains("cannot infer"));
}

#[test]
fn bad_interpolation() {
    assert!(err(&in_method("let s = $\"{}\";")).message.contains("empty"));
    assert!(err(&in_method("let s = $\"{1 +}\";")).message.contains("expected"));
}

#[test]
fn duplicate_modifier() {
    assert!(err("public public class A {}").message.contains("duplicate"));
}

#[test]
fn where_unknown_parameter() {
    assert!(err("class A<T> where U : IFoo { }").message.contains("unknown type parameter"));
}
