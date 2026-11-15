//! The `ioc` tool end to end: install / add / list / update / doctor.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn ioc(dir: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ioc"))
        .args(args)
        .current_dir(dir)
        .env("IOC_HOME", home)
        .env_remove("IOC_UPDATE_URL")
        .env_remove("IOC_CC")
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn sandbox(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iocaste-cli-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn have_cc() -> bool {
    Command::new("cc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn version_and_help() {
    let d = sandbox("version");
    let o = ioc(&d, &d, &["version"]);
    assert!(text(&o).contains(env!("CARGO_PKG_VERSION")));
    let o = ioc(&d, &d, &["help", "install"]);
    assert!(text(&o).contains("ioc install"));
    let o = ioc(&d, &d, &["help"]);
    for c in ["install", "doctor", "update", "add", "remove", "list", "clean", "config"] {
        assert!(text(&o).contains(c), "help must mention {}", c);
    }
}

#[test]
fn config_set_get_unset() {
    let d = sandbox("config");
    let home = d.join("home");
    assert!(ioc(&d, &home, &["config", "set", "update_url", "/x/latest.txt"]).status.success());
    let o = ioc(&d, &home, &["config", "get", "update_url"]);
    assert_eq!(text(&o).trim(), "/x/latest.txt");
    assert!(!ioc(&d, &home, &["config", "set", "bogus", "1"]).status.success());
    ioc(&d, &home, &["config", "unset", "update_url"]);
    assert!(!ioc(&d, &home, &["config", "get", "update_url"]).status.success());
}

#[test]
fn update_check() {
    let d = sandbox("update");
    let none = ioc(&d, &d, &["update"]);
    assert!(none.status.success());
    assert!(text(&none).contains("No update channel"));

    let newer = d.join("newer.txt");
    std::fs::write(&newer, "99.0.0\nhttps://example.invalid/ioc-99.zip\n").unwrap();
    let o = ioc(&d, &d, &["update", "--url", newer.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(10), "{}", text(&o));
    assert!(text(&o).contains("newer version"));

    let same = d.join("same.txt");
    std::fs::write(&same, format!("{}\n", env!("CARGO_PKG_VERSION"))).unwrap();
    let o = ioc(&d, &d, &["update", "--url", same.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0));
    assert!(text(&o).contains("up to date"));

    let junk = d.join("junk.txt");
    std::fs::write(&junk, "<html>\n").unwrap();
    assert_eq!(ioc(&d, &d, &["update", "--url", junk.to_str().unwrap()]).status.code(), Some(1));
}

#[test]
fn clusters_install_add_remove_and_build() {
    if !have_cc() {
        return;
    }
    let d = sandbox("clusters");
    let home = d.join("home");
    assert!(ioc(&d, &home, &["new", "mathx"]).status.success());
    std::fs::write(
        d.join("mathx/src/Program.ioc"),
        "namespace MathX;\npublic class Calc { public static square(n: int): int => n * n; }\n",
    )
    .unwrap();
    assert!(ioc(&d, &home, &["new", "app"]).status.success());
    let app = d.join("app");
    std::fs::write(
        app.join("src/Program.ioc"),
        "namespace App;\nimport Ioc.Console;\nimport MathX;\npublic class Program { public static main(args: string[]): int { Console.println($\"{Calc.square(9)}\"); return 0; } }\n",
    )
    .unwrap();

    // not installed yet
    let o = ioc(&app, &home, &["check"]);
    assert!(!o.status.success());

    let o = ioc(&app, &home, &["add", "--path", "../mathx"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(app.join("ioc.lock").is_file());
    assert!(text(&ioc(&app, &home, &["list"])).contains("installed"));

    let o = ioc(&app, &home, &["run"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("81"));

    // the cluster changes: the lock is stale until `ioc install`
    let cp = d.join("mathx/src/Program.ioc");
    let mut s = std::fs::read_to_string(&cp).unwrap();
    s.push_str("// changed\n");
    std::fs::write(&cp, s).unwrap();
    let o = ioc(&app, &home, &["build"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("ioc install"));
    assert!(ioc(&app, &home, &["install"]).status.success());
    assert!(ioc(&app, &home, &["build"]).status.success());

    // registry clusters give a clear message
    let o = ioc(&app, &home, &["add", "ioc.api", "^0.1"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("registry"));
    assert!(ioc(&app, &home, &["remove", "ioc.api"]).status.success());
    assert!(!ioc(&app, &home, &["remove", "ioc.api"]).status.success());

    assert!(ioc(&app, &home, &["clean"]).status.success());
    assert!(!app.join("build").exists());
}

#[test]
fn init_and_doctor() {
    let d = sandbox("init");
    std::fs::create_dir_all(d.join("my game")).unwrap();
    let dir = d.join("my game");
    let o = ioc(&dir, &d, &["init"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(dir.join("ioc.toml").is_file() && dir.join("src/Program.ioc").is_file());
    assert!(!ioc(&dir, &d, &["init"]).status.success());
    let o = ioc(&dir, &d, &["doctor"]);
    assert!(text(&o).contains("ioc.toml"));
}
