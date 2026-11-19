//! Toolchain commands: install, doctor, add, remove, list, clean, update, env, config, init, version.

use iocaste::manifest::{self, ClusterReq, LockEntry, Manifest, Source};
use iocaste::version::{satisfies, Version};
use iocaste::VERSION;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

// ------------------------------------------------------------------ home + config

pub fn ioc_home() -> PathBuf {
    if let Ok(h) = env::var("IOC_HOME") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    let home = env::var("HOME").or_else(|_| env::var("USERPROFILE")).unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".ioc")
}

fn config_path() -> PathBuf {
    ioc_home().join("config.toml")
}

pub fn config_get(key: &str) -> Option<String> {
    let t = fs::read_to_string(config_path()).ok()?;
    Manifest::parse(&t).ok()?.get_str("", key)
}

const CONFIG_KEYS: &[(&str, &str)] = &[
    ("cc", "C compiler used by `ioc build` (e.g. gcc, clang)"),
    ("update_url", "where `ioc update` looks for the latest version"),
];

pub fn cmd_config(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None | Some("list") => {
            println!("config file: {}", config_path().display());
            for (k, help) in CONFIG_KEYS {
                println!("  {:<11} = {:<28} # {}", k, config_get(k).map_or("(not set)".to_string(), |v| format!("\"{}\"", v)), help);
            }
            0
        }
        Some("get") => match args.get(1) {
            Some(k) => match config_get(k) {
                Some(v) => {
                    println!("{}", v);
                    0
                }
                None => 1,
            },
            None => {
                eprintln!("usage: ioc config get <key>");
                2
            }
        },
        Some("set") => {
            let (Some(k), Some(v)) = (args.get(1), args.get(2)) else {
                eprintln!("usage: ioc config set <key> <value>");
                return 2;
            };
            if !CONFIG_KEYS.iter().any(|(n, _)| n == k) {
                eprintln!("ioc: unknown setting `{}`. Known: {}", k, CONFIG_KEYS.iter().map(|x| x.0).collect::<Vec<_>>().join(", "));
                return 2;
            }
            let path = config_path();
            let old = fs::read_to_string(&path).unwrap_or_default();
            let mut lines: Vec<String> = old.lines().filter(|l| l.split('=').next().map_or(true, |x| x.trim() != k.as_str())).map(|l| l.to_string()).collect();
            lines.push(format!("{} = \"{}\"", k, v.replace('\\', "\\\\").replace('"', "\\\"")));
            if let Err(e) = fs::create_dir_all(ioc_home()).and_then(|_| fs::write(&path, lines.join("\n") + "\n")) {
                eprintln!("ioc: cannot write {}: {}", path.display(), e);
                return 2;
            }
            println!("set {} = \"{}\"", k, v);
            0
        }
        Some("unset") => {
            let Some(k) = args.get(1) else {
                eprintln!("usage: ioc config unset <key>");
                return 2;
            };
            let path = config_path();
            let old = fs::read_to_string(&path).unwrap_or_default();
            let lines: Vec<&str> = old.lines().filter(|l| l.split('=').next().map_or(true, |x| x.trim() != k.as_str())).collect();
            let _ = fs::write(&path, lines.join("\n") + "\n");
            println!("unset {}", k);
            0
        }
        Some(o) => {
            eprintln!("ioc: unknown `config` action `{}` (use list, get, set, unset)", o);
            2
        }
    }
}

// ------------------------------------------------------------------ C compiler

pub struct Cc {
    pub cmd: String,
    /// Arguments that come before the normal ones (`cc` for `zig cc`).
    pub pre: Vec<String>,
    pub version: String,
    pub managed: bool,
}

/// The compiler Iocaste manages itself. Pinned, so builds are the same on every machine.
pub const ZIG_VERSION: &str = "0.13.0";

fn zig_platform() -> Option<(&'static str, &'static str)> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => return None,
    };
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        "windows" => "windows",
        _ => return None,
    };
    Some((os, arch))
}

pub fn toolchain_dir() -> PathBuf {
    ioc_home().join("toolchain")
}

fn managed_zig_path() -> Option<PathBuf> {
    let (os, arch) = zig_platform()?;
    let exe = if cfg!(windows) { "zig.exe" } else { "zig" };
    let p = toolchain_dir().join(format!("zig-{}-{}-{}", os, arch, ZIG_VERSION)).join(exe);
    if p.exists() { Some(p) } else { None }
}

fn zig_url() -> Option<String> {
    if let Ok(u) = env::var("IOC_TOOLCHAIN_URL") {
        if !u.is_empty() {
            return Some(u);
        }
    }
    let (os, arch) = zig_platform()?;
    let ext = if os == "windows" { "zip" } else { "tar.xz" };
    Some(format!("https://ziglang.org/download/{v}/zig-{os}-{arch}-{v}.{ext}", v = ZIG_VERSION, os = os, arch = arch, ext = ext))
}

fn probe(cmd: &str, pre: &[String], managed: bool) -> Option<Cc> {
    let o = Command::new(cmd).args(pre).arg("--version").output().ok()?;
    if !o.status.success() {
        // `zig cc --version` is fine, but plain `zig` prints a bare version
        return None;
    }
    let v = String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").trim().to_string();
    Some(Cc { cmd: cmd.to_string(), pre: pre.to_vec(), version: v, managed })
}

/// Order: IOC_CC, config `cc`, the compiler ioc installed itself, then whatever the system has.
pub fn detect_cc() -> Option<Cc> {
    if let Ok(c) = env::var("IOC_CC") {
        if !c.is_empty() {
            if let Some(f) = probe(&c, &[], false) {
                return Some(f);
            }
        }
    }
    if let Some(c) = config_get("cc") {
        if let Some(f) = probe(&c, &[], false) {
            return Some(f);
        }
    }
    if let Some(z) = managed_zig_path() {
        let zs = z.to_string_lossy().to_string();
        if let Some(mut f) = probe(&zs, &["cc".to_string()], true) {
            f.version = format!("zig {} (installed by ioc)", ZIG_VERSION);
            return Some(f);
        }
    }
    for c in ["cc", "gcc", "clang"] {
        if let Some(f) = probe(c, &[], false) {
            return Some(f);
        }
    }
    None
}

/// Download the managed compiler into IOC_HOME/toolchain. Needs `curl` and `tar`, which ship with Windows 10+, macOS and Linux.
pub fn install_managed_toolchain() -> Result<(), String> {
    let Some(url) = zig_url() else {
        return Err(format!("no managed compiler is available for {} {}", std::env::consts::OS, std::env::consts::ARCH));
    };
    let dir = toolchain_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
    let archive = dir.join(if url.ends_with(".zip") { "download.zip" } else { "download.tar.xz" });
    println!("  downloading the Iocaste compiler toolchain (one time, about 80 MB)");
    println!("    from {}", url);
    let ok = Command::new("curl").args(["-fL", "--retry", "2", "--progress-bar", "-o"]).arg(&archive).arg(&url).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        let _ = fs::remove_file(&archive);
        return Err(format!("the download failed. Check your connection, or set IOC_CC to a compiler you already have.\n    (url: {})", url));
    }
    println!("  unpacking");
    let ok = Command::new("tar").arg("-xf").arg(&archive).arg("-C").arg(&dir).status().map(|s| s.success()).unwrap_or(false);
    let _ = fs::remove_file(&archive);
    if !ok {
        return Err("could not unpack the toolchain (is `tar` installed?)".to_string());
    }
    match detect_managed() {
        Some(c) => {
            println!("  ✓ installed   {}", c.version);
            Ok(())
        }
        None => Err("the toolchain was unpacked but does not run on this machine".to_string()),
    }
}

fn detect_managed() -> Option<Cc> {
    let z = managed_zig_path()?;
    let mut f = probe(&z.to_string_lossy(), &["cc".to_string()], true)?;
    f.version = format!("zig {} (installed by ioc)", ZIG_VERSION);
    Some(f)
}

/// Used by `ioc build`: find a compiler, and if there is none, fetch one. The user never has to install anything by hand.
pub fn cc_or_provision() -> Result<Cc, String> {
    if let Some(c) = detect_cc() {
        return Ok(c);
    }
    println!("ioc: no C compiler found. Setting up the built-in toolchain (first build only).");
    if env::var("IOC_NO_DOWNLOAD").is_ok() {
        return Err("IOC_NO_DOWNLOAD is set, so ioc will not fetch a compiler".to_string());
    }
    install_managed_toolchain()?;
    detect_cc().ok_or_else(|| "no compiler available after install".to_string())
}

struct PkgMgr {
    name: &'static str,
    command: &'static str,
    needs_root: bool,
}

fn package_manager() -> Option<PkgMgr> {
    let has = |c: &str| Command::new(c).arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if cfg!(target_os = "macos") {
        return Some(PkgMgr { name: "Xcode command line tools", command: "xcode-select --install", needs_root: false });
    }
    if cfg!(windows) {
        return Some(PkgMgr { name: "winget (LLVM/clang)", command: "winget install -e --id LLVM.LLVM", needs_root: false });
    }
    let table: [(&str, &str, &str); 5] = [
        ("apt-get", "apt-get install -y build-essential", "apt"),
        ("dnf", "dnf install -y gcc make", "dnf"),
        ("pacman", "pacman -S --needed --noconfirm base-devel", "pacman"),
        ("apk", "apk add build-base", "apk"),
        ("zypper", "zypper install -y gcc make", "zypper"),
    ];
    for (bin, cmd, name) in table {
        if has(bin) {
            return Some(PkgMgr { name, command: cmd, needs_root: true });
        }
    }
    None
}

fn is_root() -> bool {
    env::var("USER").map_or(false, |u| u == "root") || Command::new("id").arg("-u").output().map_or(false, |o| String::from_utf8_lossy(&o.stdout).trim() == "0")
}

/// Make sure a C compiler exists. Returns true when one is available at the end.
fn ensure_toolchain(auto: bool) -> bool {
    if let Some(cc) = detect_cc() {
        println!("  ✓ C compiler   {}  ({})", cc.cmd, cc.version);
        return true;
    }
    println!("  ✗ C compiler   not found");
    if auto {
        match install_managed_toolchain() {
            Ok(()) => return true,
            Err(e) => println!("      {}", e),
        }
    } else {
        println!("      Run `ioc install --toolchain` and ioc will download its own compiler (no admin rights needed).");
    }
    let Some(pm) = package_manager() else {
        println!("      Install gcc or clang with your system's package manager, then run `ioc doctor`.");
        return false;
    };
    let mut full = pm.command.to_string();
    if pm.needs_root && !is_root() {
        full = format!("sudo {}", full);
    }
    if !auto {
        println!("      Or install one yourself with ({}):\n          {}", pm.name, full);
        return false;
    }
    println!("      Running: {}", full);
    let ok = Command::new("sh").arg("-c").arg(&full).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        println!("      The installer did not finish. Run the command above yourself.");
        return false;
    }
    match detect_cc() {
        Some(cc) => {
            println!("  ✓ C compiler   {}  ({})", cc.cmd, cc.version);
            true
        }
        None => {
            println!("  ✗ still no C compiler found; you may need to restart your terminal");
            false
        }
    }
}

// ------------------------------------------------------------------ clusters

pub struct Resolved {
    pub name: String,
    pub version: String,
    pub source: String,
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
    pub hash: String,
}

fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let p = dir.join("ioc.toml");
    let t = fs::read_to_string(&p).map_err(|e| format!("cannot read {}: {}", p.display(), e))?;
    Manifest::parse(&t).map_err(|e| format!("{}: {}", p.display(), e))
}

fn hash_cluster(dir: &Path, files: &[PathBuf]) -> String {
    let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
    parts.push(("ioc.toml".into(), fs::read(dir.join("ioc.toml")).unwrap_or_default()));
    for f in files {
        let rel = f.strip_prefix(dir).unwrap_or(f).to_string_lossy().replace('\\', "/");
        parts.push((rel, fs::read(f).unwrap_or_default()));
    }
    let refs: Vec<(&str, &[u8])> = parts.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
    manifest::fnv(&refs)
}

fn resolve_into(req: &ClusterReq, base: &Path, out: &mut Vec<Resolved>, stack: &mut Vec<PathBuf>, errors: &mut Vec<String>) {
    match &req.source {
        Source::Registry(v) => errors.push(format!(
            "cluster `{}` ({}) comes from the cluster registry, which is not online yet (planned for v0.6.0).\n      For now, point to a folder:  ioc add {} --path <folder>",
            req.name, v, req.name
        )),
        Source::Path(p) => {
            let dir = match base.join(p).canonicalize() {
                Ok(d) => d,
                Err(_) => {
                    errors.push(format!("cluster `{}`: folder `{}` does not exist (relative to {})", req.name, p, base.display()));
                    return;
                }
            };
            if stack.contains(&dir) {
                errors.push(format!("cluster `{}` depends on itself (cycle through {})", req.name, dir.display()));
                return;
            }
            if let Some(prev) = out.iter().find(|r| r.name == req.name) {
                if prev.dir != dir {
                    errors.push(format!("two different clusters are both named `{}`: {} and {}", req.name, prev.dir.display(), dir.display()));
                }
                return;
            }
            let m = match read_manifest(&dir) {
                Ok(m) => m,
                Err(e) => {
                    errors.push(format!("cluster `{}`: {}", req.name, e));
                    return;
                }
            };
            let declared = m.name().unwrap_or_default();
            if declared != req.name {
                errors.push(format!("cluster `{}`: its ioc.toml says name = \"{}\"; the names must match", req.name, declared));
                return;
            }
            let version = m.version().unwrap_or_else(|| "0.0.0".into());
            if let Some(want) = &req.version {
                match Version::parse(&version).and_then(|v| satisfies(want, &v)) {
                    Some(true) => {}
                    Some(false) => {
                        errors.push(format!("cluster `{}` is version {} but your ioc.toml wants {}", req.name, version, want));
                        return;
                    }
                    None => {
                        errors.push(format!("cluster `{}`: cannot understand the version `{}` or `{}`", req.name, version, want));
                        return;
                    }
                }
            }
            stack.push(dir.clone());
            match m.clusters() {
                Ok(sub) => {
                    for s in &sub {
                        resolve_into(s, &dir, out, stack, errors);
                    }
                }
                Err(e) => errors.push(format!("cluster `{}`: {}", req.name, e)),
            }
            stack.pop();
            let mut files = Vec::new();
            super::collect_ioc_files(&dir.join("src"), &mut files);
            if files.is_empty() {
                errors.push(format!("cluster `{}` has no .ioc files under {}", req.name, dir.join("src").display()));
                return;
            }
            let hash = hash_cluster(&dir, &files);
            out.push(Resolved { name: req.name.clone(), version, source: format!("path:{}", p), dir, files, hash });
        }
    }
}

pub fn resolve_clusters(project: &Path) -> Result<Vec<Resolved>, Vec<String>> {
    let m = read_manifest(project).map_err(|e| vec![e])?;
    let reqs = m.clusters().map_err(|e| vec![e])?;
    let base = project.canonicalize().unwrap_or_else(|_| project.to_path_buf());
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let mut stack = Vec::new();
    for r in &reqs {
        resolve_into(r, &base, &mut out, &mut stack, &mut errors);
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

/// Source files of the installed clusters, for `ioc build` / `ioc check`. Errors say what to run.
pub fn cluster_sources(project: &Path) -> Result<Vec<PathBuf>, String> {
    if !project.join("ioc.toml").is_file() {
        return Ok(Vec::new());
    }
    let resolved = match resolve_clusters(project) {
        Ok(r) => r,
        Err(e) => return Err(e.join("\n  ")),
    };
    if resolved.is_empty() {
        return Ok(Vec::new());
    }
    let lock = fs::read_to_string(project.join("ioc.lock")).map(|t| manifest::read_lock(&t)).unwrap_or_default();
    for r in &resolved {
        match lock.iter().find(|l| l.name == r.name) {
            None => return Err(format!("cluster `{}` is not installed yet. Run `ioc install`.", r.name)),
            Some(l) if l.hash != r.hash => return Err(format!("cluster `{}` changed since the last install. Run `ioc install`.", r.name)),
            _ => {}
        }
    }
    Ok(resolved.into_iter().flat_map(|r| r.files).collect())
}

fn install_clusters(project: &Path) -> bool {
    let resolved = match resolve_clusters(project) {
        Ok(r) => r,
        Err(errs) => {
            for e in errs {
                println!("  ✗ {}", e);
            }
            return false;
        }
    };
    if resolved.is_empty() {
        println!("  ✓ clusters     none declared in ioc.toml [clusters]");
    }
    for r in &resolved {
        println!("  ✓ cluster      {} {}  ({} file(s), {})", r.name, r.version, r.files.len(), r.source);
    }
    let entries: Vec<LockEntry> = resolved
        .iter()
        .map(|r| LockEntry { name: r.name.clone(), version: r.version.clone(), source: r.source.clone(), path: r.dir.display().to_string(), hash: r.hash.clone() })
        .collect();
    if let Err(e) = fs::write(project.join("ioc.lock"), manifest::write_lock(&entries)) {
        println!("  ✗ cannot write ioc.lock: {}", e);
        return false;
    }
    println!("  ✓ ioc.lock     written ({} cluster(s))", entries.len());
    true
}

pub fn cmd_install(args: &[String]) -> i32 {
    let toolchain_only = args.iter().any(|a| a == "--toolchain");
    let yes = args.iter().any(|a| a == "--yes" || a == "-y");
    println!("Checking what is needed to build and run Iocaste programs:");
    let cc_ok = ensure_toolchain(true);
    if toolchain_only {
        return if cc_ok { 0 } else { 1 };
    }
    let mut ok = cc_ok;
    if Path::new("ioc.toml").is_file() {
        ok &= install_clusters(Path::new("."));
    } else {
        println!("  - no ioc.toml here, so no clusters to install (create a project with `ioc new <name>`)");
    }
    if ok {
        println!("\nAll set. Try `ioc run`.");
        0
    } else {
        println!("\nSome things above need attention.");
        1
    }
}

fn need_project() -> Result<String, i32> {
    match fs::read_to_string("ioc.toml") {
        Ok(t) => Ok(t),
        Err(_) => {
            eprintln!("ioc: no ioc.toml in this folder. Run inside a project (see `ioc new`, `ioc init`).");
            Err(2)
        }
    }
}

pub fn cmd_add(args: &[String]) -> i32 {
    let text = match need_project() {
        Ok(t) => t,
        Err(c) => return c,
    };
    let mut name: Option<String> = None;
    let mut path: Option<String> = None;
    let mut ver: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--path" {
            path = it.next().cloned();
            if path.is_none() {
                eprintln!("ioc: --path needs a folder");
                return 2;
            }
        } else if name.is_none() {
            name = Some(a.clone());
        } else if ver.is_none() {
            ver = Some(a.clone());
        }
    }
    let value;
    let name = match (&name, &path) {
        (n, Some(p)) => {
            let dir = Path::new(p);
            let m = match read_manifest(dir) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("ioc: {}", e);
                    return 1;
                }
            };
            let declared = m.name().unwrap_or_default();
            if let Some(n) = n {
                if *n != declared {
                    eprintln!("ioc: that folder is the cluster `{}`, not `{}`", declared, n);
                    return 1;
                }
            }
            value = format!("{{ path = \"{}\" }}", p.replace('\\', "/"));
            declared
        }
        (Some(n), None) => {
            let req = ver.unwrap_or_else(|| "*".into());
            value = format!("\"{}\"", req);
            n.clone()
        }
        (None, None) => {
            eprintln!("usage: ioc add <cluster> [version]   |   ioc add [<cluster>] --path <folder>");
            return 2;
        }
    };
    let new_text = manifest::set_cluster(&text, &name, &value);
    if let Err(e) = Manifest::parse(&new_text) {
        eprintln!("ioc: refusing to write a broken ioc.toml ({})", e);
        return 1;
    }
    if let Err(e) = fs::write("ioc.toml", new_text) {
        eprintln!("ioc: cannot write ioc.toml: {}", e);
        return 2;
    }
    println!("added `{}` to [clusters]", name);
    if install_clusters(Path::new(".")) {
        0
    } else {
        1
    }
}

pub fn cmd_remove(args: &[String]) -> i32 {
    let text = match need_project() {
        Ok(t) => t,
        Err(c) => return c,
    };
    let Some(name) = args.first() else {
        eprintln!("usage: ioc remove <cluster>");
        return 2;
    };
    match manifest::remove_cluster(&text, name) {
        None => {
            eprintln!("ioc: `{}` is not in [clusters]", name);
            1
        }
        Some(t) => {
            if let Err(e) = fs::write("ioc.toml", t) {
                eprintln!("ioc: cannot write ioc.toml: {}", e);
                return 2;
            }
            println!("removed `{}` from [clusters]", name);
            if install_clusters(Path::new(".")) {
                0
            } else {
                1
            }
        }
    }
}

pub fn cmd_list(_args: &[String]) -> i32 {
    let text = match need_project() {
        Ok(t) => t,
        Err(c) => return c,
    };
    let m = match Manifest::parse(&text) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("ioc: ioc.toml: {}", e);
            return 1;
        }
    };
    println!("{} {}", m.name().unwrap_or_else(|| "(unnamed)".into()), m.version().unwrap_or_default());
    let reqs = match m.clusters() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ioc: {}", e);
            return 1;
        }
    };
    if reqs.is_empty() {
        println!("  no clusters. Add one with `ioc add <name> --path <folder>`.");
        return 0;
    }
    let lock = fs::read_to_string("ioc.lock").map(|t| manifest::read_lock(&t)).unwrap_or_default();
    let resolved = resolve_clusters(Path::new(".")).unwrap_or_default();
    for r in &reqs {
        let what = match &r.source {
            Source::Path(p) => format!("path {}", p),
            Source::Registry(v) => format!("registry {}", v),
        };
        let status = match (resolved.iter().find(|x| x.name == r.name), lock.iter().find(|l| l.name == r.name)) {
            (Some(x), Some(l)) if x.hash == l.hash => format!("installed {}", x.version),
            (Some(_), Some(_)) => "changed: run `ioc install`".into(),
            (Some(_), None) => "not installed: run `ioc install`".into(),
            (None, _) => "cannot be resolved (see `ioc install`)".into(),
        };
        println!("  {:<20} {:<24} {}", r.name, what, status);
    }
    0
}

pub fn cmd_clean(args: &[String]) -> i32 {
    let mut removed = Vec::new();
    for p in ["build"] {
        if Path::new(p).is_dir() && fs::remove_dir_all(p).is_ok() {
            removed.push(p);
        }
    }
    if args.iter().any(|a| a == "--locks") && Path::new("ioc.lock").is_file() && fs::remove_file("ioc.lock").is_ok() {
        removed.push("ioc.lock");
    }
    if removed.is_empty() {
        println!("nothing to clean");
    } else {
        println!("removed {}", removed.join(", "));
    }
    0
}

// ------------------------------------------------------------------ doctor / version / env

pub fn cmd_doctor(_args: &[String]) -> i32 {
    let mut bad = 0;
    println!("ioc {}  ({}-{})\n", VERSION, env::consts::OS, env::consts::ARCH);
    if !ensure_toolchain(false) {
        bad += 1;
    }
    let home = ioc_home();
    println!("  {} ioc home    {}", if home.is_dir() { "✓" } else { "-" }, home.display());
    match config_get("update_url").or_else(|| env::var("IOC_UPDATE_URL").ok()) {
        Some(u) => println!("  ✓ update url   {}", u),
        None => println!("  - update url   not set (`ioc config set update_url <url-or-file>`)"),
    }
    if Path::new("ioc.toml").is_file() {
        match read_manifest(Path::new(".")) {
            Ok(m) => {
                println!("  ✓ ioc.toml     {} {}", m.name().unwrap_or_default(), m.version().unwrap_or_default());
                if let Some(req) = m.get_str("project", "ioc-version") {
                    match (Version::parse(&req), Version::parse(VERSION)) {
                        (Some(need), Some(have)) if have < need => {
                            println!("  ✗ language     project needs {} but this ioc is {}", need, have);
                            bad += 1;
                        }
                        _ => {}
                    }
                }
                match cluster_sources(Path::new(".")) {
                    Ok(_) => println!("  ✓ clusters     up to date"),
                    Err(e) => {
                        println!("  ✗ clusters     {}", e);
                        bad += 1;
                    }
                }
            }
            Err(e) => {
                println!("  ✗ ioc.toml     {}", e);
                bad += 1;
            }
        }
    } else {
        println!("  - project      no ioc.toml in this folder");
    }
    if bad == 0 {
        println!("\nEverything looks good.");
        0
    } else {
        println!("\n{} problem(s) found.", bad);
        1
    }
}

pub fn cmd_version(args: &[String]) -> i32 {
    println!("ioc {}  (Iocaste language v{})", VERSION, VERSION);
    if args.iter().any(|a| a == "--verbose" || a == "-v") {
        println!("platform   {}-{}", env::consts::OS, env::consts::ARCH);
        println!("ioc home   {}", ioc_home().display());
        match detect_cc() {
            Some(cc) => println!("c compiler {}  ({})", cc.cmd, cc.version),
            None => println!("c compiler not found"),
        }
    }
    0
}

pub fn cmd_env(_args: &[String]) -> i32 {
    println!("IOC_HOME       {}", ioc_home().display());
    println!("config file    {}", config_path().display());
    println!("IOC_CC         {}", env::var("IOC_CC").unwrap_or_else(|_| "(not set)".into()));
    println!("IOC_UPDATE_URL {}", env::var("IOC_UPDATE_URL").unwrap_or_else(|_| "(not set)".into()));
    println!("version        {}", VERSION);
    println!("platform       {}-{}", env::consts::OS, env::consts::ARCH);
    0
}

// ------------------------------------------------------------------ update

fn fetch(url: &str) -> Result<String, String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        let o = Command::new("curl")
            .args(["-fsSL", "--max-time", "8", url])
            .output()
            .map_err(|_| "cannot run `curl`; install curl, or point update_url at a local file".to_string())?;
        if !o.status.success() {
            return Err(format!("could not download {} ({})", url, String::from_utf8_lossy(&o.stderr).trim()));
        }
        Ok(String::from_utf8_lossy(&o.stdout).to_string())
    } else {
        let p = url.strip_prefix("file://").unwrap_or(url);
        fs::read_to_string(p).map_err(|e| format!("cannot read {}: {}", p, e))
    }
}

pub fn cmd_update(args: &[String]) -> i32 {
    let url = args
        .iter()
        .position(|a| a == "--url")
        .and_then(|i| args.get(i + 1).cloned())
        .or_else(|| env::var("IOC_UPDATE_URL").ok().filter(|s| !s.is_empty()))
        .or_else(|| config_get("update_url"));
    println!("installed: {}", VERSION);
    let Some(url) = url else {
        println!("\nNo update channel is configured yet (Iocaste is an alpha without a release server).");
        println!("When you have one:  ioc config set update_url <https-url-or-file>");
        println!("The file's first line is the newest version, e.g. `0.0.7-alpha`; an optional second line is a download link or note.");
        return 0;
    };
    let body = match fetch(&url) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("ioc: {}", e);
            return 1;
        }
    };
    let mut lines = body.lines();
    let latest_txt = lines.next().unwrap_or("").trim().to_string();
    let note = lines.next().unwrap_or("").trim().to_string();
    let (Some(latest), Some(mine)) = (Version::parse(&latest_txt), Version::parse(VERSION)) else {
        eprintln!("ioc: the update channel answered `{}`, which is not a version", latest_txt);
        return 1;
    };
    println!("latest:    {}", latest);
    if latest > mine {
        println!("\nA newer version is available.");
        if !note.is_empty() {
            println!("  {}", note);
        }
        println!("Self-update is manual in the alpha: download it, then rebuild with `cargo build --release`.");
        10
    } else {
        println!("\nYou are up to date.");
        0
    }
}

// ------------------------------------------------------------------ init

pub fn cmd_init(_args: &[String]) -> i32 {
    if Path::new("ioc.toml").exists() {
        eprintln!("ioc: ioc.toml already exists here");
        return 2;
    }
    let cwd = env::current_dir().ok();
    let raw = cwd.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "app".into());
    let mut name: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    if !name.chars().next().map_or(false, |c| c.is_ascii_alphabetic()) {
        name = format!("app-{}", name);
    }
    super::scaffold(Path::new("."), &name, "")
}

// ------------------------------------------------------------------ per-command help

pub fn command_help(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "install" => "ioc install [--toolchain] [--yes]\n\nChecks that a C compiler is available and installs the clusters listed in ioc.toml ([clusters]) into ioc.lock.\n  --toolchain   only check/install the C compiler\n  --yes         with --toolchain: run the system package manager command for you (never done silently)",
        "doctor" => "ioc doctor\n\nShows what is installed and what is missing: C compiler, ioc home, update channel, project manifest, clusters.",
        "add" => "ioc add <cluster> [version]\nioc add [<cluster>] --path <folder>\n\nAdds a cluster to ioc.toml and installs it. Registry clusters are not online yet; use --path.",
        "remove" => "ioc remove <cluster>\n\nRemoves a cluster from ioc.toml and refreshes ioc.lock.",
        "list" => "ioc list\n\nShows the clusters of this project and whether each is installed.",
        "update" => "ioc update [--url <url-or-file>]\n\nChecks whether a newer ioc exists. Exit code 10 means an update is available.",
        "clean" => "ioc clean [--locks]\n\nDeletes ./build (and ioc.lock with --locks).",
        "config" => "ioc config [list | get <key> | set <key> <value> | unset <key>]\n\nSettings: cc, update_url. Stored in $IOC_HOME/config.toml.",
        "env" => "ioc env\n\nPrints the paths and variables ioc uses.",
        "version" => "ioc version [--verbose]\n\nPrints the version. --verbose adds platform, ioc home and the C compiler.",
        "init" => "ioc init\n\nTurns the current folder into a project (ioc.toml + src/Program.ioc).",
        "new" => "ioc new <name>\n\nCreates a new project folder.",
        "check" => "ioc check [file.ioc...] [--syntax-only]\n\nChecks syntax, names, types and ownership. In a project it also includes installed clusters.",
        "build" => "ioc build [file.ioc...] [--emit-c] [--debug]\n\nCompiles to a native program in ./build.",
        "run" => "ioc run [file.ioc...] [-- args]\n\nBuilds and runs. Arguments after `--` go to the program.",
        _ => return None,
    })
}
