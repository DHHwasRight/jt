//! The syntax tree of a template body.

use proc_macro2::TokenStream;
use syn::{Attribute, Signature, Visibility};

/// One item of a template source: a template, or Rust passed through.
pub enum Item {
    Template(Box<Template>),
    Rust(TokenStream),
}

/// `fn Name(params) -> Json { entries }` or `fn Name(params) -> Json = value;`
pub struct Template {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub sig: Signature,
    pub body: Value,
}

/// A JSON value.
pub enum Value {
    /// JSON text known at compile time, already escaped.
    Static(String),
    /// A Rust expression whose value implements `ToJson`.
    Expr(TokenStream),
    Object(Vec<Entry>),
    Array(Vec<Elem>),
    /// `if c { value } else { value }`. A missing `else` renders `null`.
    If(IfChain<Box<Value>>),
    /// `match x { pat => value, ... }`
    Match(Match<Value>),
}

/// An object key.
pub enum Key {
    /// The quoted, escaped key, as it appears in the output.
    Static(String),
    /// `[expr]`, an expression whose value implements `JsonKey`.
    Computed(TokenStream),
}

/// What an object holds.
pub enum Entry {
    Field {
        key: Key,
        value: Value,
    },
    /// `...expr`, an expression whose value implements `JsonEntries`.
    Spread(TokenStream),
    If(IfChain<Vec<Entry>>),
    For(For<Vec<Entry>>),
    Match(Match<Vec<Entry>>),
    /// A complete `let` statement, `;` included.
    Let(TokenStream),
}

/// What an array holds.
pub enum Elem {
    Value(Value),
    If(IfChain<Vec<Elem>>),
    For(For<Vec<Elem>>),
    Match(Match<Vec<Elem>>),
    Let(TokenStream),
}

pub struct IfChain<B> {
    /// Each condition is the tokens between `if` and `{`, so `let` patterns
    /// work unchanged.
    pub branches: Vec<(TokenStream, B)>,
    pub otherwise: Option<B>,
}

pub struct For<B> {
    pub pat: TokenStream,
    pub iter: TokenStream,
    pub body: B,
}

pub struct Match<B> {
    pub scrutinee: TokenStream,
    /// Each pattern includes its guard.
    pub arms: Vec<(TokenStream, B)>,
}
