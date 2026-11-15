//! End-to-end: Iocaste source -> C -> native program -> output.
use iocaste::{cgen, parser, sema};
use std::process::Command;

fn have_cc() -> bool {
    Command::new("cc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn compile(src: &str) -> Result<String, String> {
    let ast = parser::parse(src).map_err(|d| d.message)?;
    let errs = sema::analyze(&[&ast]);
    if let Some(e) = errs.first() {
        return Err(format!("check: {}", e.diag.message));
    }
    cgen::generate(&[&ast]).map_err(|e| format!("cgen: {}", e[0].diag.message))
}

static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Returns (exit code, stdout, stderr).
fn run(src: &str) -> (i32, String, String) {
    let c = compile(src).expect("must compile");
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("iocaste-test-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    let cp = dir.join("p.c");
    let ep = dir.join("p");
    std::fs::write(&cp, c).unwrap();
    let o = Command::new("cc").args(["-O1", "-fwrapv", "-Wall", "-Werror", "-Wno-unused-function", "-Wno-unused-variable", "-Wno-unused-but-set-variable", "-Wno-return-type", "-o"]).arg(&ep).arg(&cp).arg("-lm").output().unwrap();
    assert!(o.status.success(), "C compile failed:\n{}", String::from_utf8_lossy(&o.stderr));
    let r = Command::new(&ep).output().unwrap();
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
    let e = compile("namespace T;\npublic class P { name: string; public static main(): int { return 0; } }").unwrap_err();
    assert!(e.contains("not supported by the C backend"), "{}", e);
}

#[test]
fn missing_main_is_reported() {
    let e = compile("namespace T;\npublic class P { static f(): int { return 0; } }").unwrap_err();
    assert!(e.contains("no entry point"), "{}", e);
}
