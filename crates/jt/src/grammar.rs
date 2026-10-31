//! The TextMate grammars for jt templates.
//!
//! The same grammars the VS Code extension ships (`editors/vscode/syntaxes`), here so that
//! anything rendering jt source can color it the way the editor does: a documentation site, a
//! code review tool, a highlighter such as `giallo` or `syntect`. Both build on the Rust grammar
//! by scope name (`source.rust`), so the host has to provide it too.

/// Scope name of the `.jt` file grammar.
pub const SCOPE_NAME: &str = "source.jt";

/// The `.jt` file grammar, as TextMate JSON.
pub const TEXTMATE: &str = include_str!("../grammar/jt.tmLanguage.json");

/// Scope name of the injection grammar.
pub const INJECTION_SCOPE_NAME: &str = "source.rust.jt-injection";

/// The injection grammar for Rust files: it colors the bodies of `jt::template! { … }`,
/// `jt::json!( … )`, and `jt::write_json!( … )` with the `.jt` rules. Its `injectTo` is
/// `["source.rust"]`, and it includes `source.jt`, so register both grammars.
pub const INJECTION: &str = include_str!("../grammar/jt-injection.tmLanguage.json");
