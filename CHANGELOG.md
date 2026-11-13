# Changelog

## v0.0.4-alpha (Step 4): ownership

- **Moves**: passing, assigning, returning or storing a non-copy value gives it away; using it afterwards is
  `use of moved value`. Re-assigning a `var` makes it usable again.
- **Control flow aware**: moves inside `if`/`else`/`match` arms are merged (a branch that returns/breaks does not count);
  moving something declared outside a loop from inside the loop is an error (unless the body always exits).
- **Borrows**: `let r = ref x;` / `let r = mut ref x;` keep `x` borrowed for as long as `r` is still used later.
  While borrowed: no assignment / field write / move of `x`; a `mut ref` also blocks every other use.
  One `mut ref` OR any number of `ref`. Same rule for the arguments of one call (`f(mut ref a, ref a)`).
- `return ref local` is rejected (the local is freed when the method ends).
- Copy types (never moved): numbers, `bool`, `char`, `string`, `ref T`, structs, enums whose payloads are copy
  (`Option<int>` yes, `Option<Player>` no). Classes, arrays, collections, `shared<T>` and generics move.
- `examples/ownership_errors.ioc` shows three typical mistakes with their messages. 22 new tests.

Known limits (planned): only whole local variables are tracked (moving a field out is not checked yet);
`for x in list` and `spawn { }` do not move what they use; "is the borrow still alive" is decided by whether the holder
is named later in the method (loops can hide an error); no lifetimes on returned references yet.

## v0.0.3-alpha (Step 3): names and types

- **Name resolution**: namespaces, `import` of namespaces or types, nested types, generics, visibility (`private`/`public`).
- **Type checker**: inference for `let`/`var`, numeric widening, `T` to `T?`, generics (`List<string>.add(1)` is an error),
  overloads, operators, string interpolation, `for..in` over `IEnumerable<T>`, `new shared<T>(..)`, `spawn`, `async`/`await`.
- **Result / Option rules**: `?` only inside functions returning `Result`; `??`; `match` with patterns and
  **exhaustiveness checking** for enums.
- **Declaration checks**: unknown types, inheritance, `abstract`/`virtual`/`override`, unimplemented interface members,
  `this` in static code, missing `return`, duplicate members.
- **Mutability**: assigning to a `let`, or `mut ref` of a `let`, is an error.
- **Prelude written in Iocaste** (`src/prelude.ioc`): Option, Result, Task, Channel, Console, Math, List, Dictionary, primitives.
- Enums can now be generic (`enum Option<T>`).
- `ioc check` now runs syntax then types; `--syntax-only` keeps the old behaviour.
- `examples/typed.ioc` (valid program). 45 automated tests.

Known limits: no move/borrow checking yet (v0.0.4); `features.ioc` is syntax-only (it uses undefined names on purpose);
the checker reports every error it finds, but the parser still stops at the first syntax error.

## v0.0.2-alpha (Step 2): the front-end

- Rust project `iocaste` (library + `ioc` binary), zero external dependencies, builds offline.
- **Lexer**: identifiers, reserved words, numbers (decimal, hex, binary, floats, suffixes, `_` separators),
  strings with escapes, `$"..{expr}.."` interpolation, chars, all operators, `//` and `/* */` comments, UTF-8 BOM.
- **Parser** (hand-written recursive descent): namespaces (file-scoped and block), imports, attributes,
  classes, structs, records, interfaces, enums with data, generics + `where`, fields, properties
  (`get`/`set`, expression-bodied, with initializers), methods, constructors, destructors, nested types,
  `let`/`var`, `if`/`else`, `while`, `for`, `for..in`, `return`, `break`, `continue`, `unsafe`,
  `match` with patterns and guards, `spawn`, `await`, `new`, `ref` / `mut ref`, `?`, `??`, arrays, indexing.
- **AST** with source positions on every node.
- **Diagnostics**: `error:` + file:line:col + source line + caret + `help:` hints.
  Examples: C#-style `int x;` hints at `x: int`; `null`, `try/throw` explain the Iocaste alternative;
  a missing `;` is reported at the end of the line that lacks it.
- **CLI** `ioc`: `check`, `ast`, `tokens`, `new`, `version`, `help`.
- 27 automated tests; `examples/hello.ioc` and `examples/features.ioc` parse cleanly.
- Spec updated: decisions locked (C#-family, no exceptions/null, ownership default, Rust bootstrap).

Known limits: stops at the first syntax error; no lambdas, ranges, ternary, shifts, named arguments yet.

## v0.0.1-alpha (Step 1): design

- Language specification, syntax preview (`hello.ioc`) and project manifest format (`ioc.toml`).
