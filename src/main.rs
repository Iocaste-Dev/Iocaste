//! `ioc`: the Iocaste command-line tool (v0.0.2-alpha).
//!
//! Commands: check, ast, tokens, new, version, help.

use iocaste::{dump, lexer, parser, VERSION};
use std::path::{Path, PathBuf};
use std::{env, fs, process};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    process::exit(run(&args));
}

fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None | Some("help") | Some("--help") | Some("-h") => {
            print_help();
            0
        }
        Some("version") | Some("--version") | Some("-V") => {
            println!("ioc {}  (Iocaste language v{})", VERSION, VERSION);
            0
        }
        Some("check") => cmd_check(&args[1..]),
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
    new <name>            Create a new project (ioc.toml + src/Program.ioc)
    check [file.ioc...]   Check syntax, names and types (--syntax-only). Without a file, checks every .ioc under ./src
    ast <file.ioc>        Print the syntax tree      (--ascii: plain characters, --raw: Rust debug dump)
    tokens <file.ioc>     Print the token stream
    version               Show the version
    help                  Show this help

Coming next: C backend, `ioc build` / `ioc run` (v0.0.5)."
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

fn cmd_check(args: &[String]) -> i32 {
    let syntax_only = args.iter().any(|a| a == "--syntax-only");
    let paths: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let files: Vec<PathBuf> = if paths.is_empty() {
        let src = Path::new("src");
        if !src.is_dir() {
            eprintln!("ioc: no `src` folder here. Run inside a project, or pass a file: ioc check <file.ioc>");
            return 2;
        }
        let mut v = Vec::new();
        collect_ioc_files(src, &mut v);
        if v.is_empty() {
            eprintln!("ioc: no .ioc files found under ./src");
            return 2;
        }
        v
    } else {
        paths.iter().map(|p| PathBuf::from(p.as_str())).collect()
    };

    // Phase 1: syntax.
    let mut sources = Vec::new();
    let mut asts = Vec::new();
    let mut syntax_errors = 0;
    for f in &files {
        let src = match read_source(f) {
            Ok(s) => s,
            Err(code) => return code,
        };
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
        return 1;
    }
    if syntax_only {
        println!("checked {} file(s): no syntax errors", files.len());
        return 0;
    }

    // Phase 2: names and types.
    let refs: Vec<&iocaste::ast::SourceFile> = asts.iter().collect();
    let errs = iocaste::sema::analyze(&refs);
    if errs.is_empty() {
        for f in &files {
            println!("ok      {}", f.display());
        }
        println!("\nchecked {} file(s): no errors", files.len());
        return 0;
    }
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
    1
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
            println!("  {0}/ioc.toml\n  {0}/src/Program.ioc\n  {0}/tests/", name);
            println!("\nNext:\n  cd {}\n  ioc check", name);
            0
        }
        Err(e) => {
            eprintln!("ioc: could not create project: {}", e);
            2
        }
    }
}
