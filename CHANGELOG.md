# Changelog

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
