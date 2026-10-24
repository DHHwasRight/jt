//! Procedural macros that compile jt JSON templates.
//!
//! ```ignore
//! jt::template! { pub fn User(name: &str) -> Json { "name": name } }
//! jt::load!("src/views/user.jt");   // one file, items at this position
//! jt::load_dir!("src/views");       // a tree of `pub mod`s mirroring it
//! let s = jt::json!({ "a": 1, "b": [x, y] });
//! jt::write_json!(buf, { "a": 1 });
//! ```
//!
//! JSON does not care about whitespace between tokens, so unlike an HTML
//! template, a JSON template survives Rust's tokenizer intact. That is why
//! templates can be written inline, where rust-analyzer sees the expressions
//! and errors point at the exact token.
//!
//! `load!` paths are resolved against `CARGO_MANIFEST_DIR`. Every file read is
//! also referenced by an `include_bytes!` in the expansion, which is what makes
//! cargo rebuild when a template changes.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use proc_macro::TokenStream;
use proc_macro2::{Ident, Span, TokenStream as TokenStream2};
use quote::quote;

/// Compiles templates written inline.
///
/// Any item that is not a function returning `Json` passes through unchanged.
#[proc_macro]
pub fn template(input: TokenStream) -> TokenStream {
    jt_core::or_compile_error(jt_core::expand_items(input.into())).into()
}

/// Renders a JSON value to a `String` in place.
///
/// ```ignore
/// let body = jt::json!({ "id": id, "tags": [ for t in &tags { t } ] });
/// ```
#[proc_macro]
pub fn json(input: TokenStream) -> TokenStream {
    jt_core::or_compile_error(jt_core::expand_json(input.into())).into()
}

/// Appends a JSON value to any `jt::Out`: a `Vec<u8>`, a `String`, a
/// `jt::Stream`.
///
/// ```ignore
/// jt::write_json!(buf, { "id": id });
/// ```
#[proc_macro]
pub fn write_json(input: TokenStream) -> TokenStream {
    jt_core::or_compile_error(jt_core::expand_write_json(input.into())).into()
}

/// Compiles one `.jt` file, placing its items where the macro is written.
#[proc_macro]
pub fn load(input: TokenStream) -> TokenStream {
    expand_path(input, false)
}

/// Compiles every `.jt` file under a directory into nested `pub mod`s.
///
/// `views/users/card.jt` becomes `views::users::card`.
#[proc_macro]
pub fn load_dir(input: TokenStream) -> TokenStream {
    expand_path(input, true)
}

fn expand_path(input: TokenStream, dir: bool) -> TokenStream {
    let lit: syn::LitStr = match syn::parse(input) {
        Ok(lit) => lit,
        Err(e) => return e.to_compile_error().into(),
    };
    let root = manifest_dir().join(lit.value());
    let result = if dir {
        compile_dir(&root)
    } else {
        compile_file(&root)
    };
    match result {
        Ok(tokens) => tokens.into(),
        Err(message) => syn::Error::new(lit.span(), message)
            .to_compile_error()
            .into(),
    }
}

fn manifest_dir() -> PathBuf {
    std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn compile_file(path: &Path) -> Result<TokenStream2, String> {
    let source =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let tokens = TokenStream2::from_str(&source)
        .map_err(|e| format!("{}: cannot tokenize: {e}", path.display()))?;
    let expanded = jt_core::expand_items(tokens).map_err(|e| describe(path, &source, &e))?;
    let abs = path.to_string_lossy().into_owned();
    Ok(quote! {
        const _: &[u8] = include_bytes!(#abs);
        #expanded
    })
}

/// A file or directory name as a module name.
///
/// Every character that cannot be in an identifier becomes `_`, so
/// `[id].jt` is `_id_` and `my-view.jt` is `my_view`. A leading digit gets
/// an underscore in front of it, and a name that is a keyword becomes a raw
/// identifier.
///
/// The rule is rsx's, and it is the same one `brocanth-pages` uses. Three
/// implementations agree on purpose: a page tree holds `users/[id].rs`
/// beside `users/[id].jt`, and a route and its template have to land on the
/// same module path or neither can find the other.
fn module_ident(stem: &str) -> Ident {
    let mut out = String::with_capacity(stem.len());
    for ch in stem.chars() {
        out.push(if ch.is_alphanumeric() || ch == '_' {
            ch
        } else {
            '_'
        });
    }
    if out.chars().next().is_some_and(char::is_numeric) {
        out.insert(0, '_');
    }
    if is_keyword(&out) {
        // `format_ident!` panics on `r#match`, and `match` is a name a
        // directory may legitimately have.
        return Ident::new_raw(&out, Span::call_site());
    }
    Ident::new(&out, Span::call_site())
}

fn is_keyword(word: &str) -> bool {
    const KEYWORDS: [&str; 49] = [
        "as", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false",
        "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
        "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
        "unsafe", "use", "where", "while", "async", "await", "abstract", "become", "box", "do",
        "final", "macro", "override", "priv", "typeof", "unsized", "virtual",
    ];
    KEYWORDS.contains(&word)
}

fn compile_dir(dir: &Path) -> Result<TokenStream2, String> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();

    let mut out = TokenStream2::new();
    for path in entries {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let module = module_ident(stem);
        if path.is_dir() {
            let inner = compile_dir(&path)?;
            if !inner.is_empty() {
                out.extend(quote! { pub mod #module { #inner } });
            }
        } else if path.extension().is_some_and(|e| e == "jt") {
            let inner = compile_file(&path)?;
            out.extend(quote! { pub mod #module { #inner } });
        }
    }
    Ok(out)
}

/// Formats an error in a loaded file as `path:line:col: message`.
///
/// Tokens lexed from a string inside a proc macro carry no position, so the
/// file is lexed a second time with proc-macro2's own lexer, whose spans do
/// know their line and column, and expanded again to find the error there.
/// This only happens when compilation is already failing, and stable rustc
/// expands macros on one thread, so switching proc-macro2 into fallback mode
/// for the duration cannot disturb another expansion.
fn describe(path: &Path, source: &str, error: &syn::Error) -> String {
    proc_macro2::fallback::force();
    let located = TokenStream2::from_str(source)
        .ok()
        .and_then(|tokens| jt_core::expand_items(tokens).err());
    proc_macro2::fallback::unforce();

    let error = located.as_ref().unwrap_or(error);
    let mut message = String::new();
    for e in error.clone() {
        let start = e.span().start();
        if located.is_some() && start.line > 0 {
            message.push_str(&format!(
                "{}:{}:{}: {e}\n",
                path.display(),
                start.line,
                start.column + 1
            ));
        } else {
            message.push_str(&format!("{}: {e}\n", path.display()));
        }
    }
    message.trim_end().to_owned()
}
