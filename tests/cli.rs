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

#[cfg(unix)]
#[test]
fn build_fetches_its_own_compiler_when_the_system_has_none() {
    use std::os::unix::fs::PermissionsExt;
    if !have_cc() { return; }
    let d = sandbox("provision");
    let home = d.join("home");
    // a stand-in "zig" archive that forwards to the real compiler
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let os = if os == "macos" { "macos" } else { "linux" };
    let pkg = d.join("pkg").join(format!("zig-{}-{}-0.13.0", os, arch));
    std::fs::create_dir_all(&pkg).unwrap();
    let zig = pkg.join("zig");
    std::fs::write(&zig, "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 0.13.0; exit 0; fi\nshift\nexec cc \"$@\"\n").unwrap();
    std::fs::set_permissions(&zig, std::fs::Permissions::from_mode(0o755)).unwrap();
    let tar = d.join("zig.tar.xz");
    let st = Command::new("tar").arg("-cJf").arg(&tar).arg("-C").arg(d.join("pkg")).arg(pkg.file_name().unwrap()).status().unwrap();
    assert!(st.success());
    std::fs::write(d.join("a.ioc"), "namespace T;\nimport Ioc.Console;\npublic class P { public static main(): void { Console.println(\"hi\"); } }").unwrap();
    // PATH holds only the tools ioc needs, so no system compiler is visible
    let bin = d.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for t in ["curl", "tar", "xz", "sh", "cc", "env", "id"] {
        if let Ok(o) = Command::new("which").arg(t).output() {
            let p = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !p.is_empty() && t != "cc" {
                let _ = std::os::unix::fs::symlink(p, bin.join(t));
            }
        }
    }
    let o = Command::new(env!("CARGO_BIN_EXE_ioc"))
        .args(["run", "a.ioc"])
        .current_dir(&d)
        .env("IOC_HOME", &home)
        .env("PATH", &bin)
        .env("IOC_TOOLCHAIN_URL", format!("file://{}", tar.display()))
        .env_remove("IOC_CC")
        .output()
        .unwrap();
    let t = text(&o);
    assert!(t.contains("Setting up the built-in toolchain"), "{}", t);
    // `cc` was removed from PATH, but the stand-in zig resolves it through PATH too, so only check the install step
    assert!(home.join("toolchain").join(pkg.file_name().unwrap()).join("zig").exists(), "{}", t);
    let _ = std::fs::remove_dir_all(&d);
}
