//! End-to-end: Iocaste source -> C -> native program -> output.
use iocaste::{cgen, parser, sema};
use std::process::Command;

fn have_cc() -> bool {
    Command::new("cc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn compile(src: &str) -> Result<String, String> {
    let ast = parser::parse(src).map_err(|d| d.message)?;
    let (errs, types) = sema::analyze_full(&[&ast]);
    if let Some(e) = errs.first() {
        return Err(format!("check: {}", e.diag.message));
    }
    cgen::generate(&[&ast], &types).map_err(|e| format!("cgen: {}", e[0].diag.message))
}

static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Returns (exit code, stdout, stderr).
fn run(src: &str) -> (i32, String, String) {
    run_with(src, false)
}

/// Same, but built with AddressSanitizer: any double free / use after free fails the run.
fn run_asan(src: &str) -> (i32, String, String) {
    run_with(src, true)
}

fn run_with(src: &str, asan: bool) -> (i32, String, String) {
    let c = compile(src).expect("must compile");
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("iocaste-test-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    let cp = dir.join("p.c");
    let ep = dir.join("p");
    std::fs::write(&cp, c).unwrap();
    let sanitize: &[&str] = if asan { &["-fsanitize=address,undefined", "-g"] } else { &[] };
    let o = Command::new("cc").args(sanitize).args(["-O1", "-fwrapv", "-Wall", "-Werror", "-Wno-unused-function", "-Wno-unused-variable", "-Wno-unused-but-set-variable", "-Wno-return-type", "-Wno-unused-label", "-o"]).arg(&ep).arg(&cp).arg("-lm").output().unwrap();
    assert!(o.status.success(), "C compile failed:\n{}", String::from_utf8_lossy(&o.stderr));
    let r = Command::new(&ep).env("ASAN_OPTIONS", "detect_leaks=0").output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (
        r.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&r.stdout).to_string(),
        String::from_utf8_lossy(&r.stderr).to_string(),
    )
}

fn program(body: &str, extra: &str) -> String {
    format!(
        "namespace T;\nimport Ioc.Console;\nimport Ioc.Math;\npublic class P {{\n{}\n public static main(args: string[]): int {{ {} return 0; }}\n}}",
        extra, body
    )
}

#[test]
fn hello_world() {
    if !have_cc() { return; }
    let (code, out, _) = run(&program("Console.println(\"Hello, Iocaste!\");", ""));
    assert_eq!(code, 0);
    assert_eq!(out, "Hello, Iocaste!\n");
}

#[test]
fn exit_code_comes_from_main() {
    if !have_cc() { return; }
    let src = "namespace T;\npublic class P { public static main(): int { return 7; } }";
    assert_eq!(run(src).0, 7);
}

#[test]
fn arithmetic_and_strings() {
    if !have_cc() { return; }
    let (_, out, _) = run(&program(
        "Console.println($\"{1 + 2 * 3} {10 / 4} {10.0 / 4} {7 % 3} {-5 / 2} {\"a\" + 1 + true}\");",
        "",
    ));
    assert_eq!(out, "7 2 2.5 1 -2 a1true\n");
}

#[test]
fn loops_and_calls() {
    if !have_cc() { return; }
    let (_, out, _) = run(&program(
        "var t = 0; for (var i = 1; i <= 100; i++) { t += i; } Console.println($\"{t} {fact(10)}\");",
        "static fact(n: int): long { if (n <= 1) { return 1; } return n * fact(n - 1); }",
    ));
    assert_eq!(out, "5050 3628800\n");
}

#[test]
fn overloads_pick_the_right_method() {
    if !have_cc() { return; }
    let (_, out, _) = run(&program(
        "Console.println($\"{f(2)} {f(2.5)} {f(\"x\")}\");",
        "static f(a: int): string => \"int\"; static f(a: double): string => \"double\"; static f(a: string): string => \"string\";",
    ));
    assert_eq!(out, "int double string\n");
}

#[test]
fn string_methods() {
    if !have_cc() { return; }
    let (_, out, _) = run(&program(
        "let s = \"  Hi There \"; Console.println($\"[{s.trim()}] {s.trim().toUpper()} {s.contains(\"Th\")} {s.trim().length}\");",
        "",
    ));
    assert_eq!(out, "[Hi There] HI THERE true 8\n");
}

#[test]
fn integer_overflow_wraps() {
    if !have_cc() { return; }
    let (_, out, _) = run(&program("var x: int = 2147483647; x += 1; Console.println(x.toString());", ""));
    assert_eq!(out, "-2147483648\n");
}

#[test]
fn division_by_zero_is_a_panic_not_a_crash() {
    if !have_cc() { return; }
    let (code, _, err) = run(&program("var z = 0; Console.println((5 / z).toString());", ""));
    assert_eq!(code, 101);
    assert!(err.contains("division by zero"), "{}", err);
}

#[test]
fn args_are_available() {
    if !have_cc() { return; }
    let (_, out, _) = run(&program("Console.println($\"{args.length}\");", ""));
    assert_eq!(out, "0\n");
}

#[test]
fn unsupported_features_get_a_clear_error() {
    let e = compile("namespace T;\npublic interface I { f(): int; }\npublic class P { public static main(): int { return 0; } }").unwrap_err();
    assert!(e.contains("not supported by the C backend"), "{}", e);
}

#[test]
fn missing_main_is_reported() {
    let e = compile("namespace T;\npublic class P { static f(): int { return 0; } }").unwrap_err();
    assert!(e.contains("no entry point"), "{}", e);
}

// ------------------------------------------------------------------ objects (v0.0.7)

const RES: &str = "public class Res {
    public id: int;
    public constructor(id: int) { this.id = id; Console.println($\"make {id}\"); }
    destructor() { Console.println($\"free {id}\"); }
}
";

fn objects(body: &str, extra: &str) -> String {
    format!(
        "namespace T;\nimport Ioc.Console;\n{}\npublic class P {{\n{}\n public static main(args: string[]): int {{ {} return 0; }}\n}}",
        RES, extra, body
    )
}

#[test]
fn objects_example_runs_and_frees_everything_once() {
    if !have_cc() { return; }
    let src = std::fs::read_to_string("examples/objects.ioc").unwrap();
    let (code, out, err) = run_asan(&src);
    assert_eq!(code, 0, "{}", err);
    assert_eq!(
        out,
        "Ana: 130 points, level 2\n  (freeing Ben)\nReds captain: Cy\nAna retires\n  (freeing Ana)\nend of main\n  (freeing Cy)\n"
    );
}

#[test]
fn locals_are_freed_in_reverse_order_at_scope_end() {
    if !have_cc() { return; }
    let (_, out, _) = run_asan(&objects("let a = new Res(1); let b = new Res(2); { let c = new Res(3); } Console.println(\"end\");", ""));
    assert_eq!(out, "make 1\nmake 2\nmake 3\nfree 3\nend\nfree 2\nfree 1\n");
}

#[test]
fn moved_values_are_freed_exactly_once_by_the_new_owner() {
    if !have_cc() { return; }
    let (_, out, _) = run_asan(&objects(
        "let a = new Res(1); take(a); Console.println(\"after\");",
        "static take(r: Res): void { Console.println(\"took\"); }",
    ));
    assert_eq!(out, "make 1\ntook\nfree 1\nafter\n");
}

#[test]
fn early_return_and_break_free_what_is_in_scope() {
    if !have_cc() { return; }
    let (_, out, _) = run_asan(&objects(
        "let a = new Res(1); Console.println($\"{f(true)}\"); var i = 0; while (i < 3) { let r = new Res(10 + i); i++; if (i == 2) { break; } }",
        "static f(early: bool): int { let x = new Res(5); if (early) { return 7; } return 8; }",
    ));
    assert_eq!(
        out,
        "make 1\nmake 5\nfree 5\n7\nmake 10\nfree 10\nmake 11\nfree 11\nfree 1\n"
    );
}

#[test]
fn returning_an_object_hands_it_to_the_caller() {
    if !have_cc() { return; }
    let (_, out, _) = run_asan(&objects(
        "let r = make(4); Console.println(\"got\");",
        "static make(id: int): Res { let x = new Res(id); return x; }",
    ));
    assert_eq!(out, "make 4\ngot\nfree 4\n");
}

#[test]
fn replacing_a_value_frees_the_old_one() {
    if !have_cc() { return; }
    let (_, out, _) = run_asan(&objects("var r = new Res(1); r = new Res(2); Console.println(\"end\");", ""));
    assert_eq!(out, "make 1\nmake 2\nfree 1\nend\nfree 2\n");
}

#[test]
fn a_discarded_result_is_freed() {
    if !have_cc() { return; }
    let (_, out, _) = run_asan(&objects("make(1); Console.println(\"end\");", "static make(id: int): Res { return new Res(id); }"));
    assert_eq!(out, "make 1\nfree 1\nend\n");
}

#[test]
fn fields_methods_properties_and_overloads() {
    if !have_cc() { return; }
    let src = "namespace T;
import Ioc.Console;
public class Acc {
    public total: int = 10;
    public Name: string { get; set; }
    public Double: int { get => total * 2; }
    public constructor() { Name = \"zero\"; }
    public constructor(start: int) { total = start; Name = \"s\"; }
    public add(n: int): ref Acc { total += n; return ref this; }
    public add(s: string): int => total + s.length;
}
public class P { public static main(): int {
    var a = new Acc(5); let b = new Acc();
    a.add(3).add(1); a.Name = \"x\" + a.Name;
    Console.println($\"{a.total} {a.Double} {a.Name} {b.total} {b.Name} {a.add(\"abc\")}\");
    return 0; } }";
    let (code, out, _) = run_asan(src);
    assert_eq!(code, 0);
    assert_eq!(out, "9 18 xs 10 zero 12\n");
}

#[test]
fn borrowed_objects_are_not_freed_by_the_borrower() {
    if !have_cc() { return; }
    let (_, out, _) = run_asan(&objects(
        "let a = new Res(1); look(ref a); look(ref a); Console.println(\"end\");",
        "static look(r: ref Res): void { Console.println($\"look {r.id}\"); }",
    ));
    assert_eq!(out, "make 1\nlook 1\nlook 1\nend\nfree 1\n");
}

#[test]
fn moving_out_of_a_field_is_refused_not_miscompiled() {
    let src = objects("let h = new Holder(new Res(1)); let r = h.item;", "").replace(
        "public class P {",
        "public class Holder { public item: Res; public constructor(i: Res) { item = i; } }\npublic class P {",
    );
    let e = compile(&src).unwrap_err();
    assert!(e.contains("moving an object out of a field"), "{}", e);
}

#[test]
fn returning_this_by_value_is_a_check_error() {
    let src = "namespace T;\npublic class A { public f(): A { return this; } }\npublic class P { public static main(): int { return 0; } }";
    let ast = parser::parse(src).unwrap();
    let errs = sema::analyze(&[&ast]);
    assert!(errs.iter().any(|e| e.diag.message.contains("cannot move `this`")), "{:?}", errs.iter().map(|e| &e.diag.message).collect::<Vec<_>>());
}

// ------------------------------------------------------------------ enums, match, Result / Option (v0.0.8)

#[test]
fn results_example() {
    if !have_cc() { return; }
    let src = std::fs::read_to_string("examples/results.ioc").unwrap();
    let (code, out, err) = run_asan(&src);
    assert_eq!(code, 0, "{}", err);
    assert_eq!(
        out,
        "42: adult\nnothing was typed\n'abc' is not a number\n200 is not a believable age\nnext year: 42\n'x1' is not a number\n12.56636 12 0\nfirst even: 2, none: -1\n"
    );
}

fn plain(body: &str, extra: &str) -> String {
    program(body, extra)
}

#[test]
fn enum_variants_and_match_with_literals_and_guards() {
    if !have_cc() { return; }
    let (_, out, _) = run_asan(&plain(
        "Console.println($\"{name(Color.Red)} {name(Color.Rgb(1, 2, 3))} {kind(0)} {kind(7)} {kind(-4)} {word(\"hi\")} {word(\"yo\")}\");",
        "static name(c: Color): string { return match (c) { Red => \"red\", Rgb(r, g, b) => $\"rgb {r + g + b}\" }; }
         static kind(n: int): string { return match (n) { 0 => \"zero\", x if x > 0 => \"pos\", _ => \"neg\" }; }
         static word(s: string): string { return match (s) { \"hi\" => \"hello\", _ => \"?\" }; }",
    ).replace("public class P {", "public enum Color { Red, Rgb(r: int, g: int, b: int) }\npublic class P {"));
    assert_eq!(out, "red rgb 6 zero pos neg hello ?\n");
}

#[test]
fn option_wraps_plain_values_and_coalesce_unwraps() {
    if !have_cc() { return; }
    let (_, out, _) = run_asan(&plain(
        "let a = find(3); let b = find(-1); Console.println($\"{a ?? 0} {b ?? 99} {count(a)} {count(b)}\");",
        "static find(n: int): int? { if (n > 0) { return n * 2; } return None; }
         static count(o: int?): int { return match (o) { Some(v) => v, None => -1 }; }",
    ));
    assert_eq!(out, "6 99 6 -1\n");
}

#[test]
fn question_mark_returns_the_error_early() {
    if !have_cc() { return; }
    let (_, out, _) = run_asan(&plain(
        "Console.println(show(add(\"1\", \"2\"))); Console.println(show(add(\"1\", \"x\"))); Console.println(show(add(\"y\", \"2\")));",
        "static num(s: string): Result<int, string> { let n = int.tryParse(s) ?? return Err($\"bad {s}\"); return Ok(n); }
         static add(a: string, b: string): Result<int, string> { let x = num(a)?; let y = num(b)?; return Ok(x + y); }
         static show(r: Result<int, string>): string { return match (r) { Ok(v) => $\"ok {v}\", Err(e) => $\"err {e}\" }; }",
    ));
    assert_eq!(out, "ok 3\nerr bad x\nerr bad y\n");
}

#[test]
fn try_parse_checks_the_whole_text_and_the_range() {
    if !have_cc() { return; }
    let (_, out, _) = run_asan(&plain(
        "Console.println($\"{int.tryParse(\"12\") ?? -1} {int.tryParse(\"12x\") ?? -1} {int.tryParse(\"\") ?? -1} {int.tryParse(\"99999999999\") ?? -1} {long.tryParse(\"-5\") ?? 7} {double.tryParse(\"2.5\") ?? 0.0}\");",
        "",
    ));
    assert_eq!(out, "12 -1 -1 -1 -5 2.5\n");
}

#[test]
fn enums_inside_enums_and_nested_patterns() {
    if !have_cc() { return; }
    let (_, out, _) = run_asan(&plain(
        "Console.println($\"{f(Some(5))} {f(None)} {g(Ok(Some(2)))} {g(Ok(None))} {g(Err(\"e\"))}\");",
        "static f(o: int?): int { return match (o) { Some(5) => 50, Some(_) => 1, None => 0 }; }
         static g(r: Result<int?, string>): string { return match (r) { Ok(Some(v)) => $\"v{v}\", Ok(None) => \"none\", Err(e) => e }; }",
    ));
    assert_eq!(out, "50 0 v2 none e\n");
}

#[test]
fn match_as_a_statement_with_blocks() {
    if !have_cc() { return; }
    let (_, out, _) = run_asan(&plain(
        "var total = 0; for (var i = 0; i < 4; i++) { match (i % 3) { 0 => { total += 100; } 1 => total += 10, _ => { total += 1; } } } Console.println(total.toString());",
        "",
    ));
    assert_eq!(out, "211\n");
}

#[test]
fn objects_inside_enums_are_refused_for_now() {
    let src = "namespace T;\npublic class A { }\npublic enum E { X(a: A) }\npublic class P { public static main(): int { return 0; } }";
    let e = compile(src).unwrap_err();
    assert!(e.contains("not supported by the C backend"), "{}", e);
}
