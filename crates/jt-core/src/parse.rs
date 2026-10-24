//! Parses template source into the syntax tree.
//!
//! The source is Rust tokens, so the Rust lexer has already done the hard
//! part: string literals are whole, and every `( )`, `[ ]` and `{ }` is a
//! single group. A Rust expression inside a template is therefore just the
//! token trees up to the next top-level `,` — a comma inside a call's
//! parentheses or a closure's block is invisible at that level.

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};
use syn::ext::IdentExt;
use syn::parse::discouraged::Speculative;
use syn::parse::{ParseStream, Parser};
use syn::{braced, bracketed, token, Attribute, Expr, Ident, Lit, ReturnType, Signature, Token};
use syn::{Error, Result, Type, UnOp, Visibility};

use crate::ast::*;
use crate::escape::quote_str;

/// Parses a whole template source: templates and passed-through Rust items.
pub fn items(input: TokenStream) -> Result<Vec<Item>> {
    Parser::parse2(parse_items, input)
}

/// Parses a single value, as `jt::json!` takes it.
pub fn value(input: TokenStream) -> Result<Value> {
    Parser::parse2(
        |input: ParseStream| {
            let value = parse_value(input)?;
            if !input.is_empty() {
                return Err(input.error("unexpected tokens after the value"));
            }
            Ok(value)
        },
        input,
    )
}

fn parse_items(input: ParseStream) -> Result<Vec<Item>> {
    let mut items = Vec::new();
    while !input.is_empty() {
        if let Some(template) = try_template(input)? {
            items.push(Item::Template(Box::new(template)));
        } else {
            let item: syn::Item = input.parse()?;
            items.push(Item::Rust(quote::ToTokens::into_token_stream(item)));
        }
    }
    Ok(items)
}

/// Parses a template if one starts here, leaving `input` untouched if not.
fn try_template(input: ParseStream) -> Result<Option<Template>> {
    let fork = input.fork();
    let attrs = fork.call(Attribute::parse_outer)?;
    let vis: Visibility = fork.parse()?;
    if !fork.peek(Token![fn]) {
        return Ok(None);
    }
    let sig: Signature = match fork.parse() {
        Ok(sig) => sig,
        Err(_) => return Ok(None),
    };
    if !returns_json(&sig) {
        return Ok(None);
    }
    input.advance_to(&fork);

    let (body, end) = if input.peek(token::Brace) {
        let content;
        let brace = braced!(content in input);
        (Value::Object(parse_entries(&content)?), brace.span.close())
    } else if input.peek(Token![=]) {
        input.parse::<Token![=]>()?;
        let value = parse_value(input)?;
        let semi = input.parse::<Token![;]>()?;
        (value, semi.span)
    } else {
        return Err(input.error(
            "expected a template body: `{ \"key\": value, ... }` for an object, \
             or `= value;` for any other value",
        ));
    };

    Ok(Some(Template {
        attrs,
        vis,
        sig,
        body,
        end,
    }))
}

fn returns_json(sig: &Signature) -> bool {
    match &sig.output {
        ReturnType::Type(_, ty) => match &**ty {
            Type::Path(p) => p.qself.is_none() && p.path.is_ident("Json"),
            _ => false,
        },
        ReturnType::Default => false,
    }
}

// --- values ---------------------------------------------------------------

fn parse_value(input: ParseStream) -> Result<Value> {
    if input.peek(token::Brace) {
        let content;
        braced!(content in input);
        return Ok(Value::Object(parse_entries(&content)?));
    }
    if input.peek(token::Bracket) {
        let content;
        bracketed!(content in input);
        return Ok(Value::Array(parse_elems(&content)?));
    }
    if input.peek(Token![if]) {
        return Ok(Value::If(parse_if(input, |body| {
            let value = parse_value(body)?;
            if !body.is_empty() {
                return Err(body.error(
                    "a branch of an `if` value holds one value; \
                     to emit several, put the `if` inside an object or array",
                ));
            }
            Ok(Box::new(value))
        })?));
    }
    if input.peek(Token![match]) {
        return Ok(Value::Match(parse_match(input, |arm| {
            let value = parse_value(arm)?;
            Ok((value, false))
        })?));
    }
    let tokens = until_separator(input)?;
    classify(tokens)
}

/// Decides whether an expression is a JSON literal known at compile time.
fn classify(tokens: TokenStream) -> Result<Value> {
    if tokens.is_empty() {
        return Err(Error::new(
            proc_macro2::Span::call_site(),
            "expected a value",
        ));
    }
    let expr: Expr = syn::parse2(tokens.clone()).map_err(|e| {
        // An expression cut short (`x +`) fails at the end of its own token
        // list, a position with no span. Point at the expression instead.
        if e.to_string().starts_with("unexpected end of input") {
            Error::new_spanned(&tokens, e)
        } else {
            e
        }
    })?;
    let value = match &expr {
        Expr::Lit(lit) => literal(&lit.lit, false)?,
        Expr::Unary(unary) if matches!(unary.op, UnOp::Neg(_)) => match &*unary.expr {
            Expr::Lit(lit) if matches!(lit.lit, Lit::Int(_) | Lit::Float(_)) => {
                literal(&lit.lit, true)?
            }
            _ => None,
        },
        Expr::Path(path) if path.qself.is_none() && path.path.is_ident("null") => {
            Some("null".to_owned())
        }
        _ => None,
    };
    Ok(match value {
        Some(json) => Value::Static(json),
        None => Value::Expr(tokens),
    })
}

/// The JSON text for a Rust literal, or `None` if it is not a JSON one.
fn literal(lit: &Lit, negative: bool) -> Result<Option<String>> {
    let sign = if negative { "-" } else { "" };
    Ok(Some(match lit {
        Lit::Str(s) => quote_str(&s.value()),
        Lit::Char(c) => quote_str(&c.value().to_string()),
        Lit::Bool(b) => b.value.to_string(),
        Lit::Int(i) => {
            let n: u128 = i.base10_parse()?;
            format!("{sign}{n}")
        }
        Lit::Float(f) => {
            // Format exactly as the runtime would, so a literal and the same
            // number passed as a parameter render identically.
            let mut buf = zmij::Buffer::new();
            let text = if f.suffix() == "f32" {
                let n: f32 = f.base10_parse()?;
                if !n.is_finite() {
                    return Err(Error::new(f.span(), "JSON numbers must be finite"));
                }
                buf.format_finite(if negative { -n } else { n }).to_owned()
            } else {
                let n: f64 = f.base10_parse()?;
                if !n.is_finite() {
                    return Err(Error::new(f.span(), "JSON numbers must be finite"));
                }
                buf.format_finite(if negative { -n } else { n }).to_owned()
            };
            return Ok(Some(text));
        }
        _ => return Ok(None),
    }))
}

// --- objects --------------------------------------------------------------

fn parse_entries(input: ParseStream) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    while !input.is_empty() {
        let (entry, block) = parse_entry(input)?;
        entries.push(entry);
        if input.is_empty() {
            break;
        }
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        } else if !block {
            return Err(input.error("expected `,` between object entries"));
        }
    }
    Ok(entries)
}

/// Parses one entry. The flag is true when the entry ends with a block or a
/// `;`, after which a `,` is optional.
fn parse_entry(input: ParseStream) -> Result<(Entry, bool)> {
    if input.peek(Token![...]) {
        input.parse::<Token![...]>()?;
        let expr = until_separator(input)?;
        if expr.is_empty() {
            return Err(input.error("expected an expression after `...`"));
        }
        return Ok((Entry::Spread(expr), false));
    }
    if input.peek(Token![if]) {
        return Ok((Entry::If(parse_if(input, parse_entries)?), true));
    }
    if input.peek(Token![for]) {
        return Ok((Entry::For(parse_for(input, parse_entries)?), true));
    }
    if input.peek(Token![match]) {
        let m = parse_match(input, |arm| {
            let content;
            braced!(content in arm);
            Ok((parse_entries(&content)?, true))
        })?;
        return Ok((Entry::Match(m), true));
    }
    if input.peek(Token![let]) {
        return Ok((Entry::Let(let_statement(input)?), true));
    }

    let key = if input.peek(token::Bracket) {
        let content;
        bracketed!(content in input);
        Key::Computed(content.parse()?)
    } else if input.peek(syn::LitStr) {
        let lit: syn::LitStr = input.parse()?;
        Key::Static(quote_str(&lit.value()))
    } else if input.peek(Ident::peek_any) {
        let ident = input.call(Ident::parse_any)?;
        let name = ident.unraw().to_string();
        // `name,` is shorthand for `"name": name`.
        if input.is_empty() || input.peek(Token![,]) {
            return Ok((
                Entry::Field {
                    key: Key::Static(quote_str(&name)),
                    value: Value::Expr(quote::ToTokens::into_token_stream(ident)),
                },
                false,
            ));
        }
        Key::Static(quote_str(&name))
    } else {
        return Err(input.error(
            "expected an object entry: `\"key\": value`, `key: value`, `[expr]: value`, \
             `...spread`, `if`, `for`, `match` or `let`",
        ));
    };

    let optional = input.peek(Token![?]);
    if optional {
        input.parse::<Token![?]>()?;
    }
    if input.peek(Token![::]) || !input.peek(Token![:]) {
        return Err(input.error("expected `:` after the key"));
    }
    input.parse::<Token![:]>()?;
    let value = parse_value(input)?;

    if !optional {
        return Ok((Entry::Field { key, value }, false));
    }

    // `"key"?: expr` is `if let Some(v) = &expr { "key": v }`.
    let Value::Expr(expr) = value else {
        return Err(input.error("`?:` needs an `Option` expression as its value"));
    };
    let binding = Ident::new("__jt_some", proc_macro2::Span::call_site());
    let cond = quote::quote! { let ::core::option::Option::Some(#binding) = &(#expr) };
    let field = Entry::Field {
        key,
        value: Value::Expr(quote::ToTokens::into_token_stream(binding)),
    };
    Ok((
        Entry::If(IfChain {
            branches: vec![(cond, vec![field])],
            otherwise: None,
        }),
        false,
    ))
}

// --- arrays ---------------------------------------------------------------

fn parse_elems(input: ParseStream) -> Result<Vec<Elem>> {
    let mut elems = Vec::new();
    while !input.is_empty() {
        let (elem, block) = parse_elem(input)?;
        elems.push(elem);
        if input.is_empty() {
            break;
        }
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        } else if !block {
            return Err(input.error("expected `,` between array elements"));
        }
    }
    Ok(elems)
}

fn parse_elem(input: ParseStream) -> Result<(Elem, bool)> {
    if input.peek(Token![...]) {
        // `...items` is `for x in items { x }`.
        input.parse::<Token![...]>()?;
        let iter = until_separator(input)?;
        if iter.is_empty() {
            return Err(input.error("expected an expression after `...`"));
        }
        let x = Ident::new("__jt_item", proc_macro2::Span::call_site());
        let x = quote::ToTokens::into_token_stream(x);
        return Ok((
            Elem::For(For {
                pat: x.clone(),
                iter,
                body: vec![Elem::Value(Value::Expr(x))],
            }),
            false,
        ));
    }
    if input.peek(Token![if]) {
        return Ok((Elem::If(parse_if(input, parse_elems)?), true));
    }
    if input.peek(Token![for]) {
        return Ok((Elem::For(parse_for(input, parse_elems)?), true));
    }
    if input.peek(Token![match]) {
        let m = parse_match(input, |arm| {
            let content;
            braced!(content in arm);
            Ok((parse_elems(&content)?, true))
        })?;
        return Ok((Elem::Match(m), true));
    }
    if input.peek(Token![let]) {
        return Ok((Elem::Let(let_statement(input)?), true));
    }
    Ok((Elem::Value(parse_value(input)?), false))
}

// --- control flow ---------------------------------------------------------

fn parse_if<B>(input: ParseStream, body: impl Fn(ParseStream) -> Result<B>) -> Result<IfChain<B>> {
    let mut branches = Vec::new();
    let mut otherwise = None;
    loop {
        input.parse::<Token![if]>()?;
        let cond = until_brace(input, "`if`")?;
        let content;
        braced!(content in input);
        branches.push((cond, body(&content)?));
        if !input.peek(Token![else]) {
            break;
        }
        input.parse::<Token![else]>()?;
        if input.peek(Token![if]) {
            continue;
        }
        let content;
        braced!(content in input);
        otherwise = Some(body(&content)?);
        break;
    }
    Ok(IfChain {
        branches,
        otherwise,
    })
}

fn parse_for<B>(input: ParseStream, body: impl Fn(ParseStream) -> Result<B>) -> Result<For<B>> {
    input.parse::<Token![for]>()?;
    let pat = input.step(|cursor| {
        let mut rest = *cursor;
        let mut pat = TokenStream::new();
        while let Some((tt, next)) = rest.token_tree() {
            if matches!(&tt, TokenTree::Ident(i) if i == "in") {
                return Ok((pat, rest));
            }
            pat.extend([tt]);
            rest = next;
        }
        Err(cursor.error("expected `in` in `for` loop"))
    })?;
    input.parse::<Token![in]>()?;
    let iter = until_brace(input, "`for`")?;
    let content;
    braced!(content in input);
    Ok(For {
        pat,
        iter,
        body: body(&content)?,
    })
}

/// Parses `match x { arms }`. The arm parser returns the body and whether it
/// ended with a block, after which a `,` is optional.
fn parse_match<B>(
    input: ParseStream,
    arm: impl Fn(ParseStream) -> Result<(B, bool)>,
) -> Result<Match<B>> {
    input.parse::<Token![match]>()?;
    let scrutinee = until_brace(input, "`match`")?;
    let content;
    braced!(content in input);
    let mut arms = Vec::new();
    while !content.is_empty() {
        let pat = until_fat_arrow(&content)?;
        content.parse::<Token![=>]>()?;
        let (body, block) = arm(&content)?;
        arms.push((pat, body));
        if content.is_empty() {
            break;
        }
        if content.peek(Token![,]) {
            content.parse::<Token![,]>()?;
        } else if !block {
            return Err(content.error("expected `,` after a match arm"));
        }
    }
    Ok(Match { scrutinee, arms })
}

fn let_statement(input: ParseStream) -> Result<TokenStream> {
    let mut stmt = until_separator(input)?;
    if !input.peek(Token![;]) {
        return Err(input.error("expected `;` to end the `let` statement"));
    }
    let semi: Token![;] = input.parse()?;
    quote::ToTokens::to_tokens(&semi, &mut stmt);
    Ok(stmt)
}

// --- token collection -----------------------------------------------------

/// Collects token trees up to a top-level `,` or `;`, or the end.
///
/// A turbofish is tracked so `f::<A, B>()` stays one expression. Other
/// top-level commas, such as in a bare closure's parameters, need parentheses.
fn until_separator(input: ParseStream) -> Result<TokenStream> {
    input.step(|cursor| {
        let mut rest = *cursor;
        let mut tokens = TokenStream::new();
        let mut angle = 0usize;
        let mut colons = 0u8;
        while let Some((tt, next)) = rest.token_tree() {
            if let TokenTree::Punct(p) = &tt {
                let c = p.as_char();
                if angle == 0 && (c == ',' || c == ';') {
                    break;
                }
                match c {
                    '<' if colons == 2 || angle > 0 => angle += 1,
                    '>' if angle > 0 => angle -= 1,
                    _ => {}
                }
                colons = match (c, p.spacing(), colons) {
                    (':', Spacing::Joint, 0) => 1,
                    (':', _, 1) => 2,
                    _ => 0,
                };
            } else {
                colons = 0;
            }
            tokens.extend([tt]);
            rest = next;
        }
        Ok((tokens, rest))
    })
}

/// Collects token trees up to the first top-level `{ }` group.
fn until_brace(input: ParseStream, what: &str) -> Result<TokenStream> {
    input.step(|cursor| {
        let mut rest = *cursor;
        let mut tokens = TokenStream::new();
        while let Some((tt, next)) = rest.token_tree() {
            if matches!(&tt, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace) {
                if tokens.is_empty() {
                    return Err(cursor.error(format!("expected an expression after {what}")));
                }
                return Ok((tokens, rest));
            }
            tokens.extend([tt]);
            rest = next;
        }
        Err(cursor.error(format!("expected `{{` after {what}")))
    })
}

/// Collects a match arm's pattern and guard, up to `=>`.
fn until_fat_arrow(input: ParseStream) -> Result<TokenStream> {
    input.step(|cursor| {
        let mut rest = *cursor;
        let mut tokens = TokenStream::new();
        while let Some((tt, next)) = rest.token_tree() {
            if let TokenTree::Punct(p) = &tt {
                if p.as_char() == '=' && p.spacing() == Spacing::Joint {
                    if let Some((TokenTree::Punct(q), _)) = next.token_tree() {
                        if q.as_char() == '>' {
                            return Ok((tokens, rest));
                        }
                    }
                }
            }
            tokens.extend([tt]);
            rest = next;
        }
        Err(cursor.error("expected `=>` in match arm"))
    })
}

/// Parses `target, value`, as `jt::write_json!` takes it.
pub fn target_and_value(input: TokenStream) -> Result<(TokenStream, Value)> {
    Parser::parse2(
        |input: ParseStream| {
            let target = until_separator(input)?;
            if target.is_empty() || !input.peek(Token![,]) {
                return Err(input.error("expected `jt::write_json!(out, value)`"));
            }
            input.parse::<Token![,]>()?;
            let value = parse_value(input)?;
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
            if !input.is_empty() {
                return Err(input.error("unexpected tokens after the value"));
            }
            Ok((target, value))
        },
        input,
    )
}
