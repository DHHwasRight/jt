//! Turns the text of one `.jt` file into diagnostics and symbols.
//!
//! The file is lexed with proc-macro2's own lexer, whose spans know their
//! byte offsets, and then cut into top-level items. Each item is parsed and
//! expanded on its own, so one broken template does not hide the errors and
//! the symbols of the others.
//!
//! All positions here are byte offsets into the text. The server converts
//! them to LSP positions.

use std::ops::Range;
use std::str::FromStr;

use jt_core::ast::{Entry, IfChain, Item, Key, Template, Value};
use proc_macro2::{Delimiter, Span, TokenStream, TokenTree};
use syn::spanned::Spanned;

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub range: Range<usize>,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Template,
    Function,
    Struct,
    Enum,
    Const,
    Static,
    Type,
    Trait,
    Module,
    Impl,
    Macro,
}

#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub kind: Kind,
    /// The whole item, attributes included.
    pub range: Range<usize>,
    /// The name.
    pub selection: Range<usize>,
    /// The signature on one line, as written in the source.
    pub detail: String,
    pub docs: String,
    /// What a template renders. `None` for Rust items.
    pub shape: Option<Shape>,
}

/// The JSON a template renders, as far as it is known before runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    /// The keys of the top-level object, in order. A key that is not always
    /// present ends in `?`. A spread is `...expr`, a computed key `[expr]`.
    Object(Vec<String>),
    Array,
    Value,
}

#[derive(Debug, Default)]
pub struct Analysis {
    pub diagnostics: Vec<Diagnostic>,
    pub symbols: Vec<Symbol>,
}

/// Analyzes one file.
pub fn analyze(text: &str) -> Analysis {
    let analysis = analyze_tokens(text);
    // Every lexed file stays in a thread-local source map until this call.
    // The server lexes on every keystroke, so it must clear the map.
    proc_macro2::extra::invalidate_current_thread_spans();
    analysis
}

/// Returns the Rust code that `jt::load!` generates for the file, formatted.
pub fn expand(text: &str) -> Result<String, String> {
    let result = (|| {
        let tokens = TokenStream::from_str(text).map_err(|e| format!("cannot tokenize: {e}"))?;
        if is_other_dialect(&tokens) {
            return Err("this file is for the Go or Crystal jt, not the Rust one".to_owned());
        }
        let expanded = jt_core::expand_items(tokens).map_err(|e| e.to_string())?;
        let file: syn::File = syn::parse2(expanded).map_err(|e| e.to_string())?;
        Ok(prettyplease::unparse(&file))
    })();
    proc_macro2::extra::invalidate_current_thread_spans();
    result
}

/// Returns the byte ranges of every identifier `name` in the file, outside
/// strings and comments.
pub fn references(text: &str, name: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    if let Ok(tokens) = TokenStream::from_str(text) {
        collect_idents(tokens, name, &mut out);
    }
    proc_macro2::extra::invalidate_current_thread_spans();
    out
}

fn collect_idents(tokens: TokenStream, name: &str, out: &mut Vec<Range<usize>>) {
    for tree in tokens {
        match tree {
            TokenTree::Ident(ident) if ident == name => out.push(ident.span().byte_range()),
            TokenTree::Group(group) => collect_idents(group.stream(), name, out),
            _ => {}
        }
    }
}

fn analyze_tokens(text: &str) -> Analysis {
    let mut out = Analysis::default();
    let tokens = match TokenStream::from_str(text) {
        Ok(tokens) => tokens,
        Err(e) => {
            out.diagnostics.push(Diagnostic {
                range: nonempty(e.span().byte_range(), text.len()),
                message: format!("cannot tokenize: {e}"),
            });
            return out;
        }
    };
    if is_other_dialect(&tokens) {
        return out;
    }
    for chunk in split_items(tokens) {
        analyze_chunk(text, chunk, &mut out);
    }
    out
}

/// Whether the file is written for the Go or the Crystal jt, which share
/// the `.jt` extension. Those start with `package x` or `module X` and
/// declare `template Name(..)`, neither of which is Rust.
pub fn is_other_dialect(tokens: &TokenStream) -> bool {
    let mut iter = tokens.clone().into_iter();
    let first = matches!(iter.next(), Some(TokenTree::Ident(i)) if i == "package" || i == "module");
    first && matches!(iter.next(), Some(TokenTree::Ident(_)))
        || tokens.clone().into_iter().collect::<Vec<_>>().windows(3).any(|w| {
            matches!(&w[0], TokenTree::Ident(i) if i == "template")
                && matches!(&w[1], TokenTree::Ident(_))
                && matches!(&w[2], TokenTree::Group(g) if g.delimiter() == Delimiter::Parenthesis)
        })
}

fn analyze_chunk(text: &str, chunk: Vec<TokenTree>, out: &mut Analysis) {
    let start = chunk[0].span().byte_range().start;
    let end = chunk[chunk.len() - 1].span().byte_range().end;
    let whole = start..end;
    let tokens: TokenStream = chunk.into_iter().collect();

    let items = match jt_core::parse_items(tokens.clone()) {
        Ok(items) => items,
        Err(e) => {
            push_errors(&e, &whole, None, out);
            return;
        }
    };
    let mut name = None;
    for item in &items {
        if let Some(symbol) = symbol(text, item, &whole) {
            name.get_or_insert(symbol.selection.clone());
            out.symbols.push(symbol);
        }
    }
    // Code generation has errors of its own, and the generated Rust is the
    // first place where a malformed expression inside a template shows up.
    match jt_core::expand_items(tokens) {
        Err(e) => push_errors(&e, &whole, name.as_ref(), out),
        Ok(expanded) => {
            if let Err(e) = syn::parse2::<syn::File>(expanded) {
                push_errors(&e, &whole, name.as_ref(), out);
            }
        }
    }
}

fn push_errors(
    error: &syn::Error,
    whole: &Range<usize>,
    name: Option<&Range<usize>>,
    out: &mut Analysis,
) {
    for e in error.clone() {
        let range = e.span().byte_range();
        let range = if range == (0..0) {
            // A call-site span: the end of the input, or a generated token.
            match name {
                Some(name) => name.clone(),
                None => whole.end.saturating_sub(1)..whole.end,
            }
        } else {
            range
        };
        out.diagnostics.push(Diagnostic {
            range,
            message: e.to_string(),
        });
    }
}

/// Widens an empty range to one byte, so that an editor can show it.
fn nonempty(range: Range<usize>, len: usize) -> Range<usize> {
    if range.start < range.end {
        range
    } else if range.start < len {
        range.start..range.start + 1
    } else {
        len.saturating_sub(1)..len
    }
}

/// Cuts a file into top-level items.
///
/// An item ends at a top-level `;`, or at a `{ }` group when no `=` came
/// before it in the item, so `fn T() -> Json { .. }`, `struct S { .. }` and
/// `fn T() -> Json = { .. };` each make one piece. A `;` right after a brace
/// group belongs to it (`use a::{b, c};`).
fn split_items(tokens: TokenStream) -> Vec<Vec<TokenTree>> {
    let mut items = Vec::new();
    let mut current: Vec<TokenTree> = Vec::new();
    let mut assigns = false;
    let mut iter = tokens.into_iter().peekable();
    while let Some(tree) = iter.next() {
        let ends = match &tree {
            TokenTree::Punct(p) if p.as_char() == ';' => true,
            TokenTree::Punct(p) if p.as_char() == '=' => {
                // `=` alone, not part of `==`, `=>`, `<=` or `>=`.
                let joint_prev = matches!(current.last(), Some(TokenTree::Punct(q))
                    if q.spacing() == proc_macro2::Spacing::Joint);
                if p.spacing() == proc_macro2::Spacing::Alone && !joint_prev {
                    assigns = true;
                }
                false
            }
            TokenTree::Group(g) if g.delimiter() == Delimiter::Brace && !assigns => true,
            _ => false,
        };
        let is_brace = matches!(&tree, TokenTree::Group(_));
        current.push(tree);
        if ends {
            if is_brace {
                if let Some(TokenTree::Punct(p)) = iter.peek() {
                    if p.as_char() == ';' {
                        current.push(iter.next().unwrap());
                    }
                }
            }
            items.push(std::mem::take(&mut current));
            assigns = false;
        }
    }
    if !current.is_empty() {
        items.push(current);
    }
    items
}

fn symbol(text: &str, item: &Item, whole: &Range<usize>) -> Option<Symbol> {
    match item {
        Item::Template(t) => Some(template_symbol(text, t, whole)),
        Item::Rust(tokens) => {
            let item: syn::Item = syn::parse2(tokens.clone()).ok()?;
            rust_symbol(text, &item, whole)
        }
    }
}

fn template_symbol(text: &str, t: &Template, whole: &Range<usize>) -> Symbol {
    let sig = &t.sig;
    let start = t.vis.span().byte_range();
    let start = if start == (0..0) {
        sig.fn_token.span.byte_range().start
    } else {
        start.start
    };
    let end = sig.output.span().byte_range().end;
    Symbol {
        name: sig.ident.to_string(),
        kind: Kind::Template,
        range: whole.start..t.end.byte_range().end.max(whole.end),
        selection: sig.ident.span().byte_range(),
        detail: one_line(text.get(start..end).unwrap_or_default()),
        docs: docs(&t.attrs),
        shape: Some(shape(&t.body)),
    }
}

fn rust_symbol(text: &str, item: &syn::Item, whole: &Range<usize>) -> Option<Symbol> {
    use syn::Item as I;
    let (kind, ident, attrs, head_end): (Kind, String, &[syn::Attribute], Span) = match item {
        I::Fn(f) => (
            Kind::Function,
            f.sig.ident.to_string(),
            &f.attrs,
            f.sig.span(),
        ),
        I::Struct(s) => (
            Kind::Struct,
            s.ident.to_string(),
            &s.attrs,
            s.generics.span(),
        ),
        I::Enum(e) => (Kind::Enum, e.ident.to_string(), &e.attrs, e.generics.span()),
        I::Const(c) => (Kind::Const, c.ident.to_string(), &c.attrs, c.ty.span()),
        I::Static(s) => (Kind::Static, s.ident.to_string(), &s.attrs, s.ty.span()),
        I::Type(t) => (Kind::Type, t.ident.to_string(), &t.attrs, t.ty.span()),
        I::Trait(t) => (
            Kind::Trait,
            t.ident.to_string(),
            &t.attrs,
            t.generics.span(),
        ),
        I::Mod(m) => (Kind::Module, m.ident.to_string(), &m.attrs, m.ident.span()),
        I::Impl(i) => {
            let ty = i.self_ty.span().byte_range();
            let name = one_line(text.get(ty).unwrap_or_default());
            (
                Kind::Impl,
                format!("impl {name}"),
                &i.attrs,
                i.self_ty.span(),
            )
        }
        I::Macro(m) => {
            let ident = m.ident.as_ref()?;
            (Kind::Macro, ident.to_string(), &m.attrs, ident.span())
        }
        _ => return None,
    };
    let selection = match item {
        I::Fn(f) => f.sig.ident.span(),
        I::Struct(s) => s.ident.span(),
        I::Enum(e) => e.ident.span(),
        I::Const(c) => c.ident.span(),
        I::Static(s) => s.ident.span(),
        I::Type(t) => t.ident.span(),
        I::Trait(t) => t.ident.span(),
        I::Mod(m) => m.ident.span(),
        I::Impl(i) => i.self_ty.span(),
        I::Macro(m) => m.ident.as_ref()?.span(),
        _ => return None,
    }
    .byte_range();
    // From the first token after the attributes to the end of the head.
    let start = attrs
        .iter()
        .filter(|a| matches!(a.style, syn::AttrStyle::Outer))
        .map(|a| a.span().byte_range().end)
        .max()
        .map(|end| skip_space(text, end))
        .unwrap_or(whole.start);
    let end = head_end.byte_range().end.max(selection.end);
    Some(Symbol {
        name: ident,
        kind,
        range: whole.clone(),
        selection,
        detail: one_line(text.get(start..end).unwrap_or_default()),
        docs: docs(attrs),
        shape: None,
    })
}

fn skip_space(text: &str, mut at: usize) -> usize {
    let bytes = text.as_bytes();
    while at < bytes.len() && bytes[at].is_ascii_whitespace() {
        at += 1;
    }
    at
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn docs(attrs: &[syn::Attribute]) -> String {
    let mut lines = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
            {
                let line = s.value();
                lines.push(line.strip_prefix(' ').unwrap_or(&line).to_owned());
            }
        }
    }
    lines.join("\n")
}

fn shape(body: &Value) -> Shape {
    match body {
        Value::Object(entries) => {
            let mut keys = Vec::new();
            object_keys(entries, false, &mut keys);
            Shape::Object(keys)
        }
        Value::Array(_) => Shape::Array,
        _ => Shape::Value,
    }
}

fn object_keys(entries: &[Entry], optional: bool, keys: &mut Vec<String>) {
    fn push(keys: &mut Vec<String>, key: String) {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    for entry in entries {
        match entry {
            Entry::Field { key, .. } => {
                let name = match key {
                    // The key is stored as escaped JSON text, quotes included.
                    Key::Static(json) => json[1..json.len() - 1].to_owned(),
                    Key::Computed(expr) => format!("[{}]", one_line(&expr.to_string())),
                };
                push(keys, if optional { format!("{name}?") } else { name });
            }
            Entry::Spread(expr) => push(keys, format!("...{}", one_line(&expr.to_string()))),
            Entry::If(chain) => if_keys(chain, keys),
            Entry::For(f) => object_keys(&f.body, true, keys),
            Entry::Match(m) => {
                for (_, arm) in &m.arms {
                    object_keys(arm, true, keys);
                }
            }
            Entry::Let(_) => {}
        }
    }
}

fn if_keys(chain: &IfChain<Vec<Entry>>, keys: &mut Vec<String>) {
    for (_, branch) in &chain.branches {
        object_keys(branch, true, keys);
    }
    if let Some(otherwise) = &chain.otherwise {
        object_keys(otherwise, true, keys);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = r#"// A view.
use std::fmt;

pub struct Account<'a> {
    pub id: u64,
    pub email: Option<&'a str>,
}

/// One user.
pub fn User(account: &Account<'_>) -> Json {
    "id": account.id,
    "email"?: account.email,
    if account.id > 1 { "big": true }
    ...Base(account.id),
}

fn Base(id: u64) -> Json { "base": id }

pub fn Ids(xs: &[u64]) -> Json = [ ...xs ];
"#;

    fn at<'a>(text: &'a str, range: &Range<usize>) -> &'a str {
        &text[range.clone()]
    }

    #[test]
    fn symbols_of_a_clean_file() {
        let a = analyze(USER);
        assert!(a.diagnostics.is_empty(), "{:?}", a.diagnostics);
        let names: Vec<_> = a
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
        assert_eq!(
            names,
            [
                ("Account", Kind::Struct),
                ("User", Kind::Template),
                ("Base", Kind::Template),
                ("Ids", Kind::Template),
            ]
        );
        let user = &a.symbols[1];
        assert_eq!(at(USER, &user.selection), "User");
        assert_eq!(user.detail, "pub fn User(account: &Account<'_>) -> Json");
        assert_eq!(user.docs, "One user.");
        assert_eq!(
            user.shape,
            Some(Shape::Object(vec![
                "id".into(),
                "email?".into(),
                "big?".into(),
                "...Base (account . id)".into(),
            ]))
        );
        assert!(at(USER, &user.range).starts_with("/// One user."));
        assert!(at(USER, &user.range).ends_with('}'));
        assert_eq!(a.symbols[0].detail, "pub struct Account<'a>");
        assert_eq!(a.symbols[3].shape, Some(Shape::Array));
        assert!(at(USER, &a.symbols[3].range).ends_with("];"));
    }

    #[test]
    fn one_broken_template_keeps_the_others() {
        let text =
            "fn A() -> Json { \"a\" 1 }\nfn B() -> Json { \"b\": 2 }\nfn C() -> Json { \"c\": }\n";
        let a = analyze(text);
        let names: Vec<_> = a.symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["B"]);
        assert_eq!(a.diagnostics.len(), 2, "{:?}", a.diagnostics);
        assert_eq!(a.diagnostics[0].message, "expected `:` after the key");
        assert_eq!(at(text, &a.diagnostics[0].range), "1");
        assert!(a.diagnostics[1].range.start > text.find("fn C").unwrap());
    }

    #[test]
    fn bad_expression_inside_a_template() {
        let text = "fn A(x: u8) -> Json { \"a\": x + , \"b\": 1 }\n";
        let a = analyze(text);
        assert_eq!(a.diagnostics.len(), 1, "{:?}", a.diagnostics);
        assert_eq!(at(text, &a.diagnostics[0].range), "x +");
    }

    #[test]
    fn go_and_crystal_files_are_left_alone() {
        for text in [
            "package testdata\n\ntemplate A(x: string) {\n\t\"a\": x\n}\n",
            "module Testdata\n\ntemplate A(x : String) {\n\t\"a\": x\n}\n",
        ] {
            let a = analyze(text);
            assert!(a.diagnostics.is_empty() && a.symbols.is_empty(), "{a:?}");
        }
    }

    #[test]
    fn lex_error() {
        let text = "fn A() -> Json { \"a\": \"open }\n";
        let a = analyze(text);
        assert_eq!(a.diagnostics.len(), 1);
        assert!(a.diagnostics[0].message.starts_with("cannot tokenize"));
    }

    #[test]
    fn references_skip_strings_and_comments() {
        let text = "// User\nfn User() -> Json { \"User\": 1 }\nfn P() -> Json { \"u\": User() }\n";
        let refs = references(text, "User");
        assert_eq!(refs.len(), 2);
        for r in &refs {
            assert_eq!(&text[r.clone()], "User");
        }
    }

    #[test]
    fn expansion_is_formatted_rust() {
        let code = expand("pub fn A() -> Json { \"a\": 1 }").unwrap();
        assert!(code.contains("pub struct A"), "{code}");
        assert!(code.contains("{\\\"a\\\":1}"), "{code}");
    }

    #[test]
    fn spans_do_not_leak_between_analyses() {
        for _ in 0..3 {
            let a = analyze(USER);
            assert_eq!(at(USER, &a.symbols[1].selection), "User");
        }
    }
}
