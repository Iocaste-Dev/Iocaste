# Iocaste Grammar: v0.0.2-alpha

What `ioc check` accepts today. Notation: `A?` optional, `A*` zero or more, `A | B` choice,
`'x'` literal token, `UPPER` = token class.

## Files, namespaces, declarations

```
SourceFile   = Item*
Item         = Import | Namespace | Attributes Modifiers (TypeDecl | EnumDecl)
Import       = 'import' Path ';'
Namespace    = 'namespace' Path ';' Item*          // file-scoped: covers the rest of the file
             | 'namespace' Path '{' Item* '}'
Path         = IDENT ('.' IDENT)*
Attribute    = '@' Path ('(' Args ')')?
Modifiers    = ('public'|'private'|'protected'|'internal'|'static'|'abstract'|'override'
               |'virtual'|'sealed'|'readonly'|'async'|'partial'|'unsafe')*

TypeDecl     = ('class'|'struct'|'interface') IDENT Generics? (':' Type (',' Type)*)? Where* '{' Member* '}'
             | 'record' IDENT Generics? ('(' Params ')')? (':' Type (',' Type)*)? Where* ('{' Member* '}' | ';')
Generics     = '<' IDENT (',' IDENT)* '>'
Where        = 'where' IDENT ':' Type (',' Type)*
EnumDecl     = 'enum' IDENT '{' (Variant (',' Variant)* ','?)? '}'
Variant      = IDENT ('(' Params ')')? ('=' Expr)?
```

## Members

```
Member       = Attribute* Modifiers
               ( TypeDecl | EnumDecl
               | 'constructor' '(' Params ')' Block
               | 'destructor' '(' ')' Block
               | IDENT Generics? '(' Params ')' (':' Type)? Body            // method
               | IDENT ':' Type ('=' Expr)? ';'                              // field
               | IDENT ':' Type '{' Accessor+ '}' ('=' Expr ';')?            // property
               | IDENT ':' Type '=>' Expr ';' )                              // read-only property
Accessor     = Modifiers ('get' | 'set') Body
Body         = '=>' Expr ';' | Block | ';'
Params       = (Param (',' Param)* ','?)?
Param        = IDENT ':' Type ('=' Expr)?
```

## Types

```
Type         = ('ref' | 'mut' 'ref') Type
             | Path TypeArgs? ('[' ']' | '?')*
TypeArgs     = '<' Type (',' Type)* '>'
```
`T?` is `Option<T>`. `ref T` / `mut ref T` are borrows.

## Statements

```
Block        = '{' Stmt* '}'
Stmt         = Block
             | ('let' | 'var') IDENT (':' Type)? ('=' Expr)? ';'     // needs a type or a value
             | 'if' '(' Expr ')' Stmt ('else' Stmt)?
             | 'while' '(' Expr ')' Stmt
             | 'for' '(' (LocalDecl | Expr)? ';' Expr? ';' Expr? ')' Stmt
             | 'for' '(' ('let'|'var') IDENT 'in' Expr ')' Stmt
             | 'return' Expr? ';' | 'break' ';' | 'continue' ';'
             | 'unsafe' Block
             | ('match' | 'spawn') ...                               // `;` after them is optional
             | Expr ';'
```

## Expressions (lowest to highest precedence)

```
Expr         = Assign
Assign       = Coalesce (('=' | '+=' | '-=' | '*=' | '/=' | '%=') Assign)?   // right-assoc; target: name, member or index
Coalesce     = Or ('??' Coalesce)?                                           // right-assoc
Or           = And ('||' And)*
And          = Equality ('&&' Equality)*
Equality     = Comparison (('==' | '!=') Comparison)*
Comparison   = Additive (('<' | '>' | '<=' | '>=') Additive)*
Additive     = Multiplicative (('+' | '-') Multiplicative)*
Multiplicative = Unary (('*' | '/' | '%') Unary)*
Unary        = ('-' | '!' | 'ref' | 'mut' 'ref' | 'await') Unary | Postfix
Postfix      = Primary ( '.' IDENT | '(' Args ')' | '[' Expr ']' | '?' | '++' | '--' )*
Args         = (Expr (',' Expr)* ','?)?
Primary      = INT | FLOAT | STRING | INTERPOLATED | CHAR | 'true' | 'false' | 'this' | 'base' | IDENT
             | '(' Expr ')' | '[' Args ']'
             | 'new' Type '(' Args ')'
             | 'spawn' Block
             | 'return' Expr?                                            // so `x ?? return Err(e)` works
             | 'match' '(' Expr ')' '{' (Pattern ('if' Expr)? '=>' (Expr | Block) ','?)* '}'
Pattern      = '_' | LITERAL | '-' NUMBER | IDENT | Path | Path '(' (Pattern (',' Pattern)*)? ')'
```

`expr?` is the error-propagation operator (there is no ternary `?:`).

## Literals

| Kind | Examples |
|---|---|
| Integer | `42`, `1_000_000`, `0xFF`, `0b1010`, `7u`, `9L`, `9UL` |
| Float | `1.5`, `2e3`, `3f`, `1.0d` |
| String | `"text"` with escapes `\n \t \r \0 \\ \" \' \u{1F600}` |
| Interpolated | `$"Hello {name}, 1+1={1 + 1}"`; literal braces are `{{` and `}}` |
| Char | `'a'`, `'\n'` |

Comments: `// line` and `/* block */`. Names are letters, digits, `_` (not starting with a digit).
