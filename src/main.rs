//! `ioc`: the Iocaste command-line tool.
//!
//! Commands: check, build, run, install, add, remove, list, doctor, update, clean, config, env, init, new, ast, tokens, version, help.

mod commands;

use iocaste::{dump, lexer, parser, VERSION};
use std::path::{Path, PathBuf};
use std::{env, fs, process};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    process::exit(run(&args));
}

fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None | Some("--help") | Some("-h") => {
            print_help();
            0
        }
        Some("help") => match args.get(1).and_then(|c| commands::command_help(c)) {
            Some(t) => {
                println!("{}", t);
                0
            }
            None => {
                print_help();
                0
            }
        },
        Some("version") | Some("--version") | Some("-V") => commands::cmd_version(&args[1..]),
        Some("install") => commands::cmd_install(&args[1..]),
        Some("add") => commands::cmd_add(&args[1..]),
        Some("remove") => commands::cmd_remove(&args[1..]),
        Some("list") => commands::cmd_list(&args[1..]),
        Some("doctor") => commands::cmd_doctor(&args[1..]),
        Some("update") => commands::cmd_update(&args[1..]),
        Some("clean") => commands::cmd_clean(&args[1..]),
        Some("config") => commands::cmd_config(&args[1..]),
        Some("env") => commands::cmd_env(&args[1..]),
        Some("init") => commands::cmd_init(&args[1..]),
        Some("check") => cmd_check(&args[1..]),
        Some("build") => cmd_build(&args[1..]),
        Some("run") => cmd_run(&args[1..]),
        Some("ast") => cmd_ast(&args[1..]),
        Some("tokens") => cmd_tokens(&args[1..]),
        Some("new") => cmd_new(&args[1..]),
        Some(other) => {
            eprintln!("ioc: unknown command `{}`\n", other);
            print_help();
            2
        }
    }
}

fn print_help() {
    println!(
        "ioc {VERSION}: the Iocaste toolchain

USAGE:
    ioc <command> [arguments]

COMMANDS:
  Write code
    new <name>            Create a new project folder
    init                  Turn the current folder into a project
    check [file.ioc...]   Check syntax, names, types, ownership (--syntax-only)
    build [file.ioc...]   Compile to a native program in ./build (--emit-c, --debug)
    run [file.ioc...]     Build and run; arguments after `--` go to the program
    clean                 Delete ./build

  Packages (clusters)
    install               Check the toolchain and install the clusters in ioc.toml
    add <name> [version]  Add a cluster  (or: add --path <folder>)
    remove <name>         Remove a cluster
    list                  Show this project's clusters

  The tool itself
    doctor                Show what is installed and what is missing
    version [--verbose]   Show the version
    update                Check whether a newer ioc exists
    config / env          Settings and paths

  Inspect
    ast <file.ioc>        Print the syntax tree      (--ascii, --raw)
    tokens <file.ioc>     Print the token stream

`ioc help <command>` explains one command.

Coming next: objects, enums, Result/Option and generics in the backend (v0.0.7)."
    );
}

// --------------------------------------------------------------- helpers

fn read_source(path: &Path) -> Result<String, i32> {
    fs::read_to_string(path).map_err(|e| {
        eprintln!("ioc: cannot read `{}`: {}", path.display(), e);
        2
    })
}

fn collect_ioc_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect_ioc_files(&p, out);
        } else if p.extension().map_or(false, |e| e == "ioc") {
            out.push(p);
        }
    }
}

// -------------------------------------------------------------- commands

struct Front {
    files: Vec<PathBuf>,
    sources: Vec<String>,
    asts: Vec<iocaste::ast::SourceFile>,
}

/// Find the files, then run syntax and semantic checks. Prints errors; `Err` is the exit code.
fn front_end(args: &[String], syntax_only: bool) -> Result<Front, i32> {
    let paths: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let files: Vec<PathBuf> = if paths.is_empty() {
        let src = Path::new("src");
        if !src.is_dir() {
            eprintln!("ioc: no `src` folder here. Run inside a project, or pass a file: ioc <command> <file.ioc>");
            return Err(2);
        }
        let mut v = Vec::new();
        collect_ioc_files(src, &mut v);
        if v.is_empty() {
            eprintln!("ioc: no .ioc files found under ./src");
            return Err(2);
        }
        match commands::cluster_sources(Path::new(".")) {
            Ok(extra) => v.extend(extra),
            Err(e) => {
                eprintln!("ioc: {}", e);
                return Err(1);
            }
        }
        v
    } else {
        paths.iter().map(|p| PathBuf::from(p.as_str())).collect()
    };

    let mut sources = Vec::new();
    let mut asts = Vec::new();
    let mut syntax_errors = 0;
    for f in &files {
        let src = read_source(f)?;
        match parser::parse(&src) {
            Ok(ast) => asts.push(ast),
            Err(d) => {
                syntax_errors += 1;
                eprint!("{}", d.render(&f.display().to_string(), &src));
                eprintln!();
            }
        }
        sources.push(src);
    }
    if syntax_errors > 0 {
        eprintln!("checked {} file(s): {} with syntax errors", files.len(), syntax_errors);
        return Err(1);
    }
    if !syntax_only {
        let refs: Vec<&iocaste::ast::SourceFile> = asts.iter().collect();
        let errs = iocaste::sema::analyze(&refs);
        if !errs.is_empty() {
            for e in &errs {
                if e.file < files.len() {
                    eprint!("{}", e.diag.render(&files[e.file].display().to_string(), &sources[e.file]));
                } else {
                    eprint!("{}", e.diag.render("<prelude>", iocaste::sema::PRELUDE));
                    eprintln!("(this is a bug in the built-in prelude, not in your code)");
                }
                eprintln!();
            }
            eprintln!("checked {} file(s): {} error(s)", files.len(), errs.len());
            return Err(1);
        }
    }
    Ok(Front { files, sources, asts })
}

fn cmd_check(args: &[String]) -> i32 {
    let syntax_only = args.iter().any(|a| a == "--syntax-only");
    match front_end(args, syntax_only) {
        Err(c) => c,
        Ok(f) => {
            if !syntax_only {
                for p in &f.files {
                    println!("ok      {}", p.display());
                }
                println!();
            }
            println!(
                "checked {} file(s): no {}",
                f.files.len(),
                if syntax_only { "syntax errors" } else { "errors" }
            );
            0
        }
    }
}

/// Name of the program: `name = "..."` in ./ioc.toml, else the first file's name.
fn program_name(files: &[PathBuf], explicit_files: bool) -> String {
    if !explicit_files {
        if let Ok(t) = fs::read_to_string("ioc.toml") {
            for line in t.lines() {
                let l = line.trim();
                if let Some(rest) = l.strip_prefix("name") {
                    if let Some(v) = rest.trim_start().strip_prefix('=') {
                        let n = v.trim().trim_matches('"').trim();
                        if !n.is_empty() {
                            return n.to_string();
                        }
                    }
                }
            }
        }
    }
    files[0].file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "program".into())
}

/// Generate C, compile it, and return the path of the executable.
fn build(args: &[String]) -> Result<PathBuf, i32> {
    let explicit = args.iter().any(|a| !a.starts_with("--"));
    let front = front_end(args, false)?;
    let refs: Vec<&iocaste::ast::SourceFile> = front.asts.iter().collect();
    let c = match iocaste::cgen::generate(&refs) {
        Ok(c) => c,
        Err(errs) => {
            for e in &errs {
                eprint!("{}", e.diag.render(&front.files[e.file].display().to_string(), &front.sources[e.file]));
                eprintln!();
            }
            eprintln!("build failed: {} error(s)", errs.len());
            return Err(1);
        }
    };
    let name = program_name(&front.files, explicit);
    let dir = Path::new("build");
    if let Err(e) = fs::create_dir_all(dir) {
        eprintln!("ioc: cannot create `build`: {}", e);
        return Err(2);
    }
    let c_path = dir.join(format!("{}.c", name));
    if let Err(e) = fs::write(&c_path, &c) {
        eprintln!("ioc: cannot write `{}`: {}", c_path.display(), e);
        return Err(2);
    }
    if args.iter().any(|a| a == "--emit-c") {
        println!("wrote   {}", c_path.display());
        return Ok(c_path);
    }
    let exe = dir.join(if cfg!(windows) { format!("{}.exe", name) } else { name.clone() });
    let cc = commands::detect_cc().map(|c| c.cmd).unwrap_or_else(|| "cc".into());
    let opt: &[&str] = if args.iter().any(|a| a == "--debug") { &["-O0", "-g"] } else { &["-O2"] };
    let out = std::process::Command::new(&cc)
        .args(opt)
        .args(["-fwrapv", "-w", "-o"])
        .arg(&exe)
        .arg(&c_path)
        .arg("-lm")
        .output();
    match out {
        Ok(o) if o.status.success() => {
            println!("built   {}", exe.display());
            Ok(exe)
        }
        Ok(o) => {
            eprintln!("ioc: the C compiler failed. This is a bug in the Iocaste backend; the generated C is in {}", c_path.display());
            eprint!("{}", String::from_utf8_lossy(&o.stderr));
            Err(1)
        }
        Err(e) => {
            eprintln!("ioc: cannot run the C compiler `{}`: {}", cc, e);
            eprintln!("     Install gcc or clang, or set IOC_CC to your compiler.");
            Err(2)
        }
    }
}

fn cmd_build(args: &[String]) -> i32 {
    match build(args) {
        Ok(_) => 0,
        Err(c) => c,
    }
}

fn cmd_run(args: &[String]) -> i32 {
    // arguments after `--` go to the program
    let split = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let (ours, theirs) = args.split_at(split);
    let theirs: Vec<&String> = theirs.iter().skip(1).collect();
    let exe = match build(ours) {
        Ok(e) => e,
        Err(c) => return c,
    };
    let status = std::process::Command::new(&exe).args(theirs).status();
    match status {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("ioc: cannot run `{}`: {}", exe.display(), e);
            2
        }
    }
}

fn cmd_ast(args: &[String]) -> i32 {
    let ascii = args.iter().any(|a| a == "--ascii");
    let raw = args.iter().any(|a| a == "--raw");
    let Some(file) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("usage: ioc ast <file.ioc> [--ascii] [--raw]");
        return 2;
    };
    let path = Path::new(file);
    let src = match read_source(path) {
        Ok(s) => s,
        Err(c) => return c,
    };
    match parser::parse(&src) {
        Ok(ast) => {
            if raw {
                println!("{:#?}", ast);
            } else {
                print!("{}", dump::dump(&ast, ascii));
            }
            0
        }
        Err(d) => {
            eprint!("{}", d.render(file, &src));
            1
        }
    }
}

fn cmd_tokens(args: &[String]) -> i32 {
    let Some(file) = args.first() else {
        eprintln!("usage: ioc tokens <file.ioc>");
        return 2;
    };
    let src = match read_source(Path::new(file)) {
        Ok(s) => s,
        Err(c) => return c,
    };
    match lexer::lex(&src) {
        Ok(toks) => {
            for t in toks {
                println!("{:>4}:{:<3} {:?}", t.span.line, t.span.col, t.tok);
            }
            0
        }
        Err(d) => {
            eprint!("{}", d.render(file, &src));
            1
        }
    }
}

fn pascal_case(name: &str) -> String {
    name.split(|c| c == '-' || c == '_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut cs = p.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn cmd_new(args: &[String]) -> i32 {
    let Some(name) = args.first() else {
        eprintln!("usage: ioc new <project-name>");
        return 2;
    };
    let valid = name.chars().next().map_or(false, |c| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        eprintln!("ioc: invalid project name `{}`. Use letters, digits, `-` and `_`, starting with a letter.", name);
        return 2;
    }
    let root = Path::new(name);
    if root.exists() {
        eprintln!("ioc: `{}` already exists", name);
        return 2;
    }
    scaffold(root, name, name)
}

/// Write the files of a new project into `root`. `shown` is the folder name used in messages ("" = here).
fn scaffold(root: &Path, name: &str, shown: &str) -> i32 {
    let ns = pascal_case(name);

    let manifest = format!(
        r#"# Iocaste project manifest

[project]
name = "{name}"
version = "0.1.0"
authors = []
description = ""
license = "MIT"
ioc-version = "{VERSION}"        # minimum language version required
entry = "src/Program.ioc"         # file containing the static main
kind = "app"                      # app | library | cluster

[target]
default = "native"                # native | desktop | web | android

[build]
debug-backend = "cranelift"       # fast compile
release-backend = "llvm"          # fastest runtime
incremental = true
parallel = true

[clusters]                        # packages (WebSocket, API, game dev, ...)
# "ioc.api"       = "^0.1"
# "ioc.websocket" = "^0.1"
# "ioc.game"      = "^0.1"

[extensions]                      # community compiler extensions
"#
    );

    let program = format!(
        r#"namespace {ns};

import Ioc.Console;

public class Program {{
    public static main(args: string[]): int {{
        Console.println("Hello from Iocaste!");
        return 0;
    }}
}}
"#
    );

    let result = (|| -> std::io::Result<()> {
        fs::create_dir_all(root.join("src"))?;
        fs::create_dir_all(root.join("tests"))?;
        fs::write(root.join("ioc.toml"), manifest)?;
        fs::write(root.join("src").join("Program.ioc"), program)?;
        fs::write(root.join(".gitignore"), "build/\nclusters/\n")?;
        Ok(())
    })();

    match result {
        Ok(()) => {
            println!("Created project `{}`", name);
            let p = if shown.is_empty() { String::new() } else { format!("{}/", shown) };
            println!("  {0}ioc.toml\n  {0}src/Program.ioc\n  {0}tests/", p);
            if shown.is_empty() {
                println!("\nNext:\n  ioc run");
            } else {
                println!("\nNext:\n  cd {}\n  ioc run", shown);
            }
            0
        }
        Err(e) => {
            eprintln!("ioc: could not create project: {}", e);
            2
        }
    }
}
