# Iocaste (`.ioc`): v0.0.3-alpha

A high-level, OOP-first language that compiles to native code with Rust-style ownership instead of a
garbage collector. C#-family syntax with TypeScript-style types (`name: Type`).

**Status:** the front-end exists (lexer, parser, AST, diagnostics). It can read and validate Iocaste code,
but it cannot run it yet. Type checking is v0.0.3, `ioc run` is v0.0.5. See `docs/` for the full spec and roadmap.

## What you need

- [Rust](https://rustup.rs) (stable, 2021 edition or newer). Nothing else: no crates to download.

## Build

```bash
cargo build --release          # produces target/release/ioc   (ioc.exe on Windows)
cargo test                     # runs the 27 tests
```

Optional: put `target/release` on your PATH, or `cargo install --path .` to get `ioc` everywhere.

## Use

```bash
ioc new my-app                 # creates my-app/ioc.toml and my-app/src/Program.ioc
cd my-app
ioc check                      # syntax-checks every .ioc under ./src

ioc check examples/hello.ioc   # check one file
ioc ast examples/hello.ioc     # print the syntax tree (add --ascii on terminals without box characters)
ioc tokens examples/hello.ioc  # print the token stream
```

Errors look like this:

```
error: expected `(` or `:` after the member name, found identifier `x`
 --> Program.ioc:1:15
  |
1 | class A { int x; }
  |               ^
  = help: in Iocaste the type comes after the name, e.g. `count: int` or `run(): void`
```

## Your first program

`src/Program.ioc`:

```ioc
namespace MyApp;

import Ioc.Console;

public class Program {
    public static main(args: string[]): int {
        Console.println("Hello from Iocaste!");
        return 0;
    }
}
```

Everything lives in a class. The entry point is `static main`. Errors are values (`Result<T, E>`), and
there is no `null` (`Option<T>`, written `T?`). See `examples/hello.ioc` for ownership, `match`, `spawn`
and channels, and `examples/features.ioc` for every construct the parser knows.

## Project layout

```
iocaste/
  Cargo.toml
  src/
    lib.rs        library entry
    diag.rs       positions + error rendering
    lexer.rs      source -> tokens
    ast.rs        syntax tree types
    parser.rs     tokens -> syntax tree
    dump.rs       tree printer (ioc ast)
    main.rs       the `ioc` command
  tests/parser.rs
  examples/       hello.ioc, features.ioc
  docs/           language specification
  GRAMMAR.md      what the parser accepts
  CHANGELOG.md
```

## Where this is going

| Version | Goal |
|---|---|
| v0.0.2 | Lexer, parser, AST, `ioc check/ast/tokens/new` |
| **v0.0.3 (now)** | Name resolution + type checker (`ioc check` finds undefined names, type errors, non-exhaustive `match`) |
| v0.0.4 | Borrow checker (ownership, `ref`, `mut ref`) |
| v0.0.5 | C backend: Hello World runs, `ioc build` / `ioc run` |
| v0.0.6 | Standard library core |
| v0.1.0 | Generics, interfaces, Result/Option, tasks and channels |
| v0.2.0+ | Desktop, Web (WASM), Android, backends, Clusters (API, WebSocket, game), IDE support |
