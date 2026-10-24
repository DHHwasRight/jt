# jt

Compile-time JSON templates for Rust. A template is a Rust function whose
body is JSON with holes. A proc macro compiles it into straight-line writes.
Nothing is interpreted at runtime, and no intermediate tree is built.

```rust
jt::template! {
    pub fn User(u: &Account) -> Json {
        "id": u.id,
        "name": u.name,
        "email"?: u.email,                       // omitted when None
        "roles": [ for r in &u.roles { r } ],
        if u.admin { "admin": true }
        "kind": "user",
    }

    pub fn Page(users: &[Account], next: Option<&str>) -> Json {
        "data": [ for u in users { User(u) } ],
        "next"?: next,
    }
}

let body: String = jt::to_string(&Page(&accounts, None));
jt::to_writer(&mut socket, &Page(&accounts, None))?;
```

It takes ideas from [brocanth/jt](https://github.com/brocanth/jt) (the Go
original: JSON templates compiled to writer calls) and from `rsx` (Rust
items in a template file, struct-plus-function components, buffer reuse).

## Why it is fast

| Technique | Effect |
| --- | --- |
| Static text is escaped and merged at compile time | `{"id":`, `,"name":` and every constant value are one `memcpy` each, however many keys they span |
| Commas are resolved at compile time | A runtime flag exists only in a container that has conditionals or loops |
| Borrowed inputs | Templates take `&str`, `&[T]` and `&Struct`. No string is copied into the template |
| SWAR string escaping | 8 bytes per step, with unchecked writes into reserved space. Clean text escapes at about 10 GiB/s |
| Learned size hint | `to_string` allocates once, at the size of the previous render |
| Buffer pool | `to_pooled` and `to_writer` reuse thread-local buffers. A warm pool allocates nothing |
| Chunked streaming | `to_writer` gives the sink 8 KiB writes, not one write per token |

The output is byte-for-byte what `serde_json` produces for the same data:
the same escapes, the same integer and float formats (`itoa`, `zmij`), and
`null` for non-finite floats. Each benchmark asserts this before it measures.

## Benchmarks

`cargo bench -p jt --bench vs_serde`, Apple M4 Pro, median of 100 samples.
`serde_json` 1.0.154 with `#[derive(Serialize)]` on the same data.

| Case | serde_json | jt | Speed-up |
| --- | ---: | ---: | ---: |
| Flat object, 8 fields | 80 ns | 23 ns | 3.5x |
| Nested object (Notion page) | 189 ns | 77 ns | 2.5x |
| 10 users, `to_string` | 3.69 µs | 1.09 µs | 3.4x |
| 1000 users, `to_string` (~290 KB) | 224 µs | 104 µs | 2.2x |
| 1000 users, reused `Vec` | 241 µs | 109 µs | 2.2x |
| 1000 users, `io::Write` | 444 µs | 116 µs | 3.8x |
| 1000 users, `io::Write` + `BufWriter` | 333 µs | 116 µs | 2.9x |
| 11 KB string, no escapes | 4.67 µs | 1.09 µs | 4.3x |
| 9 KB string, dense escapes | 5.46 µs | 2.15 µs | 2.5x |
| 8 KB string, non-ASCII | 2.22 µs | 0.55 µs | 4.0x |

Run-to-run noise on a laptop is about ±30 % in absolute times. The ratios
are stable. The `io::Write` sink copies every byte it gets, as a socket does:
`io::sink()` drops bytes without touching them and would make many small
writes look free.

## Getting started

```toml
[dependencies]
jt = { path = "crates/jt" }
```

Three ways to write templates:

```rust
jt::template! { pub fn User(name: &str) -> Json { "name": name } }  // inline
jt::load!("src/views/user.jt");                                     // one file
jt::load_dir!("src/views");                                         // a tree of `pub mod`s
```

Inline templates get full rust-analyzer support, because the expressions are
ordinary tokens in a Rust file. `.jt` files keep large templates apart from
code. Paths are relative to `CARGO_MANIFEST_DIR`. Each loaded file is also
referenced by an `include_bytes!`, so cargo rebuilds when the file changes.
An error in a file names the exact line and column:
`src/views/user.jt:3:9: expected ':' after the key`.

For a one-off document, use a value macro:

```rust
let s: String = jt::json!({ "id": id, "tags": [ for t in &tags { t } ] });
jt::write_json!(buf, { "ok": true });          // appends to any jt::Out
```

## Syntax

A template source is Rust. A function that returns `Json` is a template. Any
other item (`use`, `struct`, `fn`, `impl`) passes through unchanged.

```rust
pub fn Name(params) -> Json { entries }   // the body is an object
pub fn Name(params) -> Json = value;      // any other value
```

### Values

| Syntax | Output |
| --- | --- |
| `"text"`, `42`, `-1.5`, `true`, `null` | the literal, escaped at compile time |
| `expr` | any Rust expression whose type implements `ToJson` |
| `{ entries }` | an object |
| `[ elements ]` | an array |
| `if c { v } else { v }` | one of two values; no `else` gives `null` |
| `match x { p => v, ... }` | one value per arm |

An expression ends at the next top-level `,`. Commas inside `( )`, `[ ]`,
`{ }` and a turbofish `f::<A, B>()` do not count. A bare closure with two
parameters needs parentheses.

### Object entries

| Syntax | Meaning |
| --- | --- |
| `"key": value` | a field |
| `key: value` | a field with an identifier key; `r#type: 1` gives `"type"` |
| `name,` | shorthand for `"name": name` |
| `"key"?: opt` | the field only when `opt` is `Some`; the value is the inner value |
| `[expr]: value` | a computed key; `expr` implements `JsonKey` (strings, chars, integers) |
| `...expr` | spread the entries of a map, a `&[(K, V)]`, an `Option`, or another object template |
| `if c { entries } else { entries }` | conditional entries; `if let` works |
| `for pat in iter { entries }` | repeated entries, usually with computed keys |
| `match x { p => { entries } }` | entries per arm |
| `let pat = expr;` | a binding for the entries after it |

Entries are separated by `,`. After a block (`if`, `for`, `match`) or a
`let` statement the comma is optional. A trailing comma is allowed.

### Array elements

| Syntax | Meaning |
| --- | --- |
| `value` | one element |
| `...iter` | one element per item of `iter` |
| `if c { elements }` | conditional elements |
| `for pat in iter { elements }` | elements per item; the body can hold several |
| `match x { p => { elements } }` | elements per arm |
| `let pat = expr;` | a binding |

In an array, `if` and `match` produce elements, not a value. `[ if c { 1 } ]`
is `[1]` or `[]`.

### Composition

A template call is an expression, so templates nest:

```rust
pub fn Page(users: &[Account]) -> Json {
    "data": [ for u in users { User(u) } ],
}
```

A template whose body is an object also implements `JsonEntries`, so one
template can extend another:

```rust
pub fn Base(id: u64) -> Json { "id": id, "created": "2026-01-01" }
pub fn Admin(id: u64) -> Json { ...Base(id), "admin": true }
```

### Escape hatches

| Type | Output |
| --- | --- |
| `jt::Raw(&str)` | pre-rendered JSON, written verbatim and not checked |
| `jt::Display(x)` | `x`'s `Display` output as a JSON string, with no temporary `String` |
| `jt::Serde(&x)` | any `serde::Serialize` value, through `serde_json` (feature `serde`) |

## What gets generated

For `pub fn User(name: &str, age: u32) -> Json { "name": name, "age": age, "kind": "user" }`:

```rust
#[derive(Clone)]
pub struct User<'jt> { pub name: &'jt str, pub age: u32 }

pub fn User<'jt>(name: &'jt str, age: u32) -> User<'jt> { User { name, age } }

impl<'jt> User<'jt> {
    pub fn write<O: jt::Out + ?Sized>(o: &mut O, name: &str, age: u32) {
        lit(o, "{\"name\":");
        ToJson::to_json(&name, o);
        lit(o, ",\"age\":");
        ToJson::to_json(&age, o);
        lit(o, ",\"kind\":\"user\"}");
    }
}

impl ToJson for User<'_> { /* calls write */ }
impl JsonEntries for User<'_> { /* the same, without braces */ }
```

`User::write(&mut buf, "Ada", 36)` skips the struct and writes directly. The
struct form is for composing and for the `jt::to_*` functions. When it
renders, each field is passed with `Clone::clone`, which is a plain copy for
references and numbers. Take parameters by reference.

Print the expansion of any file with
`cargo run -p jt-core --example expand -- path/to/file.jt`.

### Comma handling

Before each element the generator knows one of three states: nothing written
yet, something written, or unknown. Only the unknown state reads a runtime
flag. This template:

```rust
pub fn Cond(name: &str, x: bool, xs: &[i32]) -> Json {
    "a": 1,
    "name": name,
    if x { "x": 1 }
    "z": { "deep": [1, 2] },
    "xs": [ for v in xs { v } ],
}
```

compiles to three static writes, one conditional static write, and one flag
for the loop (simplified):

```rust
lit(o, "{\"a\":1,\"name\":");
ToJson::to_json(&name, o);
if x { lit(o, ",\"x\":1"); }
lit(o, ",\"z\":{\"deep\":[1,2]},\"xs\":[");
let mut f = true;
for v in xs {
    if !f { lit(o, ","); }
    ToJson::to_json(&v, o);
    f = false;
    o.checkpoint();
}
lit(o, "]}");
```

## Output API

| Function | Result | Allocation |
| --- | --- | --- |
| `jt::to_string(&t)` | `String` | one, sized from the previous render |
| `jt::to_vec(&t)` | `Vec<u8>` | one, sized from the previous render |
| `jt::to_pooled(&t)` | `PooledBuf` (derefs to `[u8]`, `.as_str()`) | none once the pool is warm |
| `jt::write_into(&mut buf, &t)` | appends to your buffer | none if `buf` has capacity |
| `jt::to_writer(w, &t)` | streams into any `io::Write` | none once the pool is warm |
| `t.to_json(&mut out)` | appends to any `jt::Out` (`Vec<u8>`, `String`, `PooledBuf`, `Stream`) | none |

`Stream` writes to its sink at checkpoints: after each array element and loop
iteration, once 8 KiB are buffered. Memory stays bounded by one chunk plus
one element. Write errors are sticky, and `Stream::finish` returns the first
one, so the generated code contains no error checks.

The pool is thread-local: no lock, no atomic. It keeps up to 16 buffers per
thread and frees any buffer that grew past 4 MiB.

## `ToJson` implementations

`str`, `String`, `Cow<str>`, `char`, `bool`, `()`, all integers, `f32`,
`f64`, `Option<T>`, `&T`, `&mut T`, `Box`, `Rc`, `Arc`, `[T]`, `[T; N]`,
`Vec`, `VecDeque`, `HashSet`, `BTreeSet`, tuples up to 8, and `HashMap` /
`BTreeMap` with `JsonKey` keys. Implement `ToJson` for your own types, or
write a template for them.

## Limits

- A parameter type with a hidden lifetime needs it written out:
  `&Account<'_>`, not `&Account`. The macro cannot see that `Account` has a
  lifetime parameter.
- `impl Trait` parameters are not supported. Use a generic parameter.
- Parameters must be plain names. Destructure with `let` in the body.
- A `{` in value position is always an object. Wrap a Rust block expression
  in parentheses: `( { let x = 1; x } )`.

## Editor support

`crates/jt-lsp` is a language server for `.jt` files. It reports parser
errors as you type, and gives hover, go to definition, references,
completion, symbols and a `jt/expand` request that returns the generated
Rust. `editors/vscode` is the VS Code extension for it.

```
mise run vscode   # build, package and install the extension
mise run lsp      # only install jt-lsp into ~/.cargo/bin, for other editors
```

Any LSP client works: run `jt-lsp` over stdio for the `jt` language. Types
are not checked in the editor; cargo checks them when it builds.

## Crates

| Crate | Content |
| --- | --- |
| `jt` | runtime: `ToJson`, `Out`, `Stream`, pool, output functions; re-exports the macros |
| `jt-core` | parser and code generator, testable without a compiler |
| `jt-macros` | `template!`, `load!`, `load_dir!`, `json!`, `write_json!` |
| `jt-lsp` | language server for `.jt` files |

## Development

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo bench -p jt --bench vs_serde
```
