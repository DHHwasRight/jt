//! Parser and code generator for jt JSON templates.
//!
//! The proc macros in `jt-macros` are thin wrappers around the three
//! `expand_*` functions here. Keeping the logic in a normal library makes it
//! testable without a compiler in the loop.

mod ast;
mod codegen;
mod escape;
mod parse;

use proc_macro2::TokenStream;
use quote::ToTokens;

pub use escape::quote_str;

/// Expands template source: templates become code, other items pass through.
pub fn expand_items(input: TokenStream) -> syn::Result<TokenStream> {
    let mut out = TokenStream::new();
    for item in parse::items(input)? {
        match item {
            ast::Item::Template(t) => out.extend(codegen::template(&t)?),
            ast::Item::Rust(tokens) => out.extend(tokens),
        }
    }
    Ok(out)
}

/// Expands `jt::json!(value)` into an expression that evaluates to a `String`.
pub fn expand_json(input: TokenStream) -> syn::Result<TokenStream> {
    Ok(codegen::json(&parse::value(input)?))
}

/// Expands `jt::write_json!(out, value)` into a block that appends to `out`.
pub fn expand_write_json(input: TokenStream) -> syn::Result<TokenStream> {
    let (target, value) = parse::target_and_value(input)?;
    Ok(codegen::write_json(&target, &value))
}

/// Renders any expansion error as `compile_error!` tokens.
pub fn or_compile_error(result: syn::Result<TokenStream>) -> TokenStream {
    result.unwrap_or_else(|e| e.to_compile_error().into_token_stream())
}
