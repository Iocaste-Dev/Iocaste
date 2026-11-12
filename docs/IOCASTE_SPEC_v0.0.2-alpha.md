# Iocaste Language Specification: v0.0.2 alpha (Step 2: Decisions locked, front-end built)

> Changes since v0.0.1: section 9 decisions are now locked; `iocc` front-end (lexer, parser, AST,
> `ioc check/ast/tokens/new`) exists in Rust. See `CHANGELOG.md`, `GRAMMAR.md` and `README.md`.

File extension: `.ioc`  |  Compiler: `iocc`  |  CLI: `ioc`  |  Packages: **Clusters**

---

## 1. The big decision: low-level or high-level?

**Decision: Iocaste is a high-level language with systems-level performance.**
It sits in the same tier as Go, Swift and Rust: you write OOP code at a high level,
but it compiles ahead-of-time to native machine code with no GC and no VM.

| Option | Verdict |
|---|---|
| Low-level (C-like, manual everything) | Rejected. Conflicts with "easy to understand" and "extremely OOP". |
| High-level with a VM/GC (C#, Java) | Rejected. Conflicts with "extremely fast like Go" and "memory like Rust". |
| **High-level syntax + native AOT + ownership memory** | **Chosen.** |

Low-level power is still reachable through an `unsafe { }` block and `extern` FFI,
but 95% of code never needs them.

---

## 2. Honest conflicts in the requirements (and how v0.0.1 resolves them)

You asked for several things that pull against each other. Resolving them early saves months.

### 2.1 "Superset of C#" vs "Memory management like Rust"
A true superset means every valid C# program compiles. C# assumes a garbage collector
and freely shared references; Rust-style ownership forbids that by default. Both cannot hold at once.

**Resolution (default for v0.0.1): Iocaste is *C#-family*, not a strict superset.**
- ~90% of C# syntax is accepted (classes, interfaces, properties, generics, enums, records, `async/await`, LINQ-style methods, namespaces).
- Semantics differ where ownership demands it (see section 4).
- A future `#compat csharp` migration mode may auto-insert `shared<T>` where a C# file relies on GC aliasing.

> **Decision locked (v0.0.2): C#-family, not a strict superset.**

### 2.2 "Like Go" speed
Go is fast mainly because of a quick compiler, cheap concurrency and a small runtime. Iocaste will beat Go on raw runtime (no GC pauses), and target Go-like compile times through a fast debug backend (see section 6).

### 2.3 "Extremely OOP" + Rust ownership
Compatible. Classes are **owned** by default (one owner, moves on assignment). Sharing is explicit and opt-in.

---

## 3. Language design

### 3.1 Syntax rules (TypeScript + C#)
- Types come **after** names (TypeScript style): `let age: int = 30;`
- Type inference with `let` (immutable) and `var` (mutable).
- Classes, interfaces, properties, `namespace`, access modifiers from C#: `public`, `private`, `protected`, `internal`.
- Statements end with `;`. Blocks use `{ }`.
- String interpolation: `$"Hello {name}"`.
- Everything lives in a class. Program entry is `static main`.
- No exceptions. Errors are values: `Result<T, E>` with the `?` operator (fast, predictable).
- `null` does not exist. Use `Option<T>` (`T?` is sugar for it).

### 3.2 Primitive types
`bool, byte, sbyte, short, ushort, int, uint, long, ulong, float, double, char, string, void`
Plus `usize` / `isize` for indexing.

### 3.3 OOP features (v0.0.1 target)
Classes, abstract classes, interfaces, inheritance (single) + interface (multiple), properties
(`get`/`set`), constructors, static members, generics with constraints, enums (with data),
records, operator overloading, partial classes, attributes (`@Name`).

### 3.4 Concurrency (Go-like)
- `async` / `await` for I/O.
- `spawn { ... }` starts a lightweight task (green thread) on a work-stealing scheduler.
- `Channel<T>` for message passing.
- Ownership prevents data races at compile time ("send" only what you own).

---

## 4. Memory model (Rust-like, easier to read)

Rust's power with fewer sigils. Rules:

| Concept | Iocaste | Rust analogue |
|---|---|---|
| Owned value (default) | `let a = new Player();` | `Box<T>` / owned |
| Move | `let b = a;` (a is now invalid) | move |
| Immutable borrow | `ref Player` | `&T` |
| Mutable borrow | `mut ref Player` | `&mut T` |
| Explicit copy | `a.clone()` | `.clone()` |
| Shared ownership | `shared<T>` (ref-counted) | `Rc` / `Arc` |
| Non-owning link | `weak<T>` | `Weak` |
| Escape hatch | `unsafe { }` | `unsafe` |

- **Lifetimes are inferred.** The compiler only asks for annotations (`'a`) in rare, ambiguous cases.
- Memory is freed deterministically at scope end via `Drop` (`destructor`).
- Simple value types (`int`, `struct`) are copied automatically.

---

## 5. Built-ins vs Clusters

**Built-in (ships with the compiler):**
1. Core language + standard library (`Ioc.Core`, `Ioc.IO`, `Ioc.Collections`, `Ioc.Text`, `Ioc.Time`, `Ioc.Math`, `Ioc.Threading`).
2. **Desktop applications** (`Ioc.Desktop`): windows, widgets, layouts, events, native backends (Windows, macOS, Linux).
3. **Web applications** (`Ioc.Web`): compile to WebAssembly + generated JS glue for the browser; server-side HTTP hosting for web apps.

**Clusters (installable packages, built last as you specified):**
- `ioc.websocket`: WebSocket server and client
- `ioc.api`: REST/HTTP API framework (routing, JSON, middleware)
- `ioc.game`: game development (loop, ECS, 2D/3D rendering, input, audio)

**Android:** you listed it without saying built-in or cluster. Proposed: an **Android target** (`ioc build --target android`) backed by NDK native output plus a thin generated Kotlin shell, scheduled after Web. *(Confirm or change.)*

---

## 6. Compiler architecture (built for fast compilation)

```
 .ioc source
    |  Lexer          -> tokens
    |  Parser         -> AST
    |  Name resolve   -> symbol tables
    |  Type checker   -> typed AST
    |  Borrow checker -> ownership validation
    |  IoIR           -> Iocaste intermediate representation
    |-- Debug backend:   Cranelift (very fast compile, ok runtime)
    |-- Release backend: LLVM      (slow compile, maximum runtime speed)
    |-- Web backend:     WebAssembly
    |-- Bootstrap backend (v0.0.x only): C code -> system C compiler
```

Fast-compile rules baked into the design:
- No header files, no preprocessor, no textual includes.
- Each module compiles independently and in parallel; incremental cache keyed by content hash.
- Generics are monomorphized but deduplicated; no template-style blow-up.
- Debug builds use Cranelift by default.

**Bootstrap strategy:** the first compiler (`iocc` v0.0.x) will be written in **Rust**, which is the natural host for a compiler with this memory model. Once the language is stable (around v0.5), rewrite `iocc` in Iocaste itself (self-hosting).

---

## 7. Toolchain: what a developer needs to start a project

| Tool | Purpose |
|---|---|
| `iocc` | the compiler |
| `ioc` | the one CLI: `ioc new`, `ioc build`, `ioc run`, `ioc test`, `ioc fmt`, `ioc check`, `ioc cluster add/remove/update` |
| `ioc.toml` | project manifest (see `ioc.toml` example file) |
| `ioc.lock` | exact cluster versions (auto-generated) |
| `Ioc.Core` standard library | collections, IO, text, time, math, threading |
| Cluster registry (later) | where community Clusters are published |
| `ioc-lsp` (low priority) | language server for VS Code and JetBrains IntelliSense |
| Extensions API | community compiler analyzers/macros/codegen shipped inside Clusters, plus editor extensions |

Standard project layout:
```
my-project/
  ioc.toml
  ioc.lock
  src/
    Program.ioc
  tests/
  clusters/        (cached packages)
  build/           (output)
```

---

## 8. Roadmap

| Version | Goal |
|---|---|
| **v0.0.1-alpha (now)** | Design spec, syntax sample, manifest format |
| v0.0.2 | Lexer + parser + AST dump (`ioc check`) |
| v0.0.3 | Type checker + basic types, classes, methods |
| v0.0.4 | Borrow checker (ownership, `ref`, `mut ref`) |
| v0.0.5 | C backend: "Hello World" runs; `ioc run` works |
| v0.0.6 | Standard library core (collections, strings, IO) |
| v0.1.0 | Generics, interfaces, Result/Option, tasks and channels |
| v0.2.0 | `Ioc.Desktop` built-in |
| v0.3.0 | `Ioc.Web` built-in (WASM) |
| v0.4.0 | Android target |
| v0.5.0 | Cranelift/LLVM backends, self-hosting begins |
| v0.6.0 | Cluster package manager + registry |
| v0.7.0 | Clusters: `ioc.api`, `ioc.websocket` |
| v0.8.0 | Cluster: `ioc.game` |
| v0.9.0 | `ioc-lsp` + VS Code / JetBrains plugins |
| v1.0.0 | Stable release |

---

## 9. Decisions (locked in v0.0.2)

1. **C#-family** syntax and semantics, not a strict C# superset (section 2.1). Locked.
2. **No exceptions, no `null`.** `Result<T, E>`, `Option<T>` (`T?`) and the `?` operator (section 3.1). Locked.
   The parser already rejects `null`, `try`, `throw`, `catch`, `finally` with a helpful message.
3. **Ownership by default, `shared<T>` opt-in** (section 4). Locked.
4. **Android is a build target**, not a Cluster (section 5). Assumed from the v0.0.1 proposal; change it any time before v0.4.
5. **The compiler is written in Rust.** Locked. v0.0.2 has no external crate dependencies.

## 10. Reserved words (v0.0.2)

```
namespace import class interface enum struct record
public private protected internal static abstract override virtual sealed readonly async partial unsafe
constructor destructor
let var if else while for in return break continue new match spawn true false this base ref mut await
null throw try catch finally        (reserved only to give a helpful error)
```
`get`, `set` and `where` are contextual keywords (usable as names elsewhere).

## 11. Not in the parser yet (planned)

Lambdas / closures, ranges, `if` as an expression, ternary `?:`, shift operators (`<<`, `>>`),
prefix `++`/`--`, explicit generic arguments on calls (`f<int>(x)`), named arguments, lifetimes (`'a`),
`switch`, `extern` / FFI declarations, operator-overload declarations, `where` on methods,
and error recovery (the parser stops at the first error).
