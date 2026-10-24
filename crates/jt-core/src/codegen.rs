//! Turns the syntax tree into Rust.
//!
//! The generated code is a flat list of writes. Two things make it fast.
//!
//! **Static text is merged.** Every key, punctuation mark and literal value is
//! escaped here, at compile time, and adjacent static pieces are joined into
//! one string literal. `{"a":1,"b":` is one `memcpy`, however many entries it
//! spans.
//!
//! **Commas are resolved at compile time where possible.** Before each
//! element the generator knows one of three things: nothing has been written
//! to the container yet (`First`), something has (`Rest`), or it depends on
//! which branches ran (`Unknown`). Only `Unknown` needs a runtime flag, and
//! only containers that reach it declare one. A plain object with no
//! conditionals has its commas baked into its static strings.
//!
//! The flag is only read in the `Unknown` state, so it only has to be correct
//! there. It is written wherever control flow moves from a known state into
//! an unknown one: at the end of each `if` branch whose outcome differs from
//! its siblings, before a loop whose iterations cannot know whether they run
//! first, and around a spread.

use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote, ToTokens};
use syn::visit_mut::{self, VisitMut};
use syn::{Error, FnArg, GenericParam, Lifetime, LifetimeParam, Pat, Result, Type};

use crate::ast::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    First,
    Rest,
    Unknown,
}

enum Op {
    /// Static JSON text.
    Lit(String),
    /// Code that writes output, or depends on what was written.
    Code(TokenStream),
    /// Code that neither writes nor reads output: `let` statements and flag
    /// updates. Static text may move across it.
    Quiet(TokenStream),
}

/// A block of generated statements under construction.
#[derive(Default)]
struct Ops(Vec<Op>);

impl Ops {
    fn lit(&mut self, s: &str) {
        for op in self.0.iter_mut().rev() {
            match op {
                Op::Quiet(_) => continue,
                Op::Lit(text) => {
                    text.push_str(s);
                    return;
                }
                Op::Code(_) => break,
            }
        }
        self.0.push(Op::Lit(s.to_owned()));
    }

    fn code(&mut self, tokens: TokenStream) {
        self.0.push(Op::Code(tokens));
    }

    fn quiet(&mut self, tokens: TokenStream) {
        self.0.push(Op::Quiet(tokens));
    }

    /// Bytes of static text this block always writes.
    fn static_len(&self) -> usize {
        self.0
            .iter()
            .map(|op| match op {
                Op::Lit(s) => s.len(),
                _ => 0,
            })
            .sum()
    }

    fn into_tokens(self) -> TokenStream {
        let out = out_ident();
        let mut tokens = TokenStream::new();
        for op in self.0 {
            match op {
                Op::Lit(s) => tokens.extend(quote! { ::jt::__private::lit(#out, #s); }),
                Op::Code(t) | Op::Quiet(t) => tokens.extend(t),
            }
        }
        tokens
    }
}

fn out_ident() -> Ident {
    Ident::new("__jt_o", Span::call_site())
}

/// The comma state of one object or array.
struct Container {
    /// The flag as a place expression: a local, or `(*__jt_first)`.
    flag: TokenStream,
    used: bool,
}

/// Something that generates the contents of a container.
trait Body {
    fn gen(&self, g: &mut Gen, ops: &mut Ops, state: State, c: &mut Container) -> State;
}

impl Body for Vec<Entry> {
    fn gen(&self, g: &mut Gen, ops: &mut Ops, state: State, c: &mut Container) -> State {
        g.entries(ops, self, state, c)
    }
}

impl Body for Vec<Elem> {
    fn gen(&self, g: &mut Gen, ops: &mut Ops, state: State, c: &mut Container) -> State {
        g.elems(ops, self, state, c)
    }
}

#[derive(Default)]
struct Gen {
    next_flag: usize,
}

impl Gen {
    fn container(&mut self) -> (Ident, Container) {
        let ident = format_ident!("__jt_f{}", self.next_flag);
        self.next_flag += 1;
        let c = Container {
            flag: ident.to_token_stream(),
            used: false,
        };
        (ident, c)
    }

    /// Writes the separator an element needs in `state`.
    fn separator(&self, ops: &mut Ops, state: State, c: &mut Container) {
        match state {
            State::First => {}
            State::Rest => ops.lit(","),
            State::Unknown => {
                c.used = true;
                let flag = &c.flag;
                let out = out_ident();
                ops.code(quote! {
                    if !#flag { ::jt::__private::lit(#out, ","); }
                });
            }
        }
    }

    /// Makes the runtime flag match a known state.
    fn sync(&self, ops: &mut Ops, state: State, c: &mut Container) {
        let value = match state {
            State::First => true,
            State::Rest => false,
            State::Unknown => return,
        };
        c.used = true;
        let flag = &c.flag;
        ops.quiet(quote! { ::jt::__private::set(&mut #flag, #value); });
    }

    fn value(&mut self, ops: &mut Ops, value: &Value) {
        let out = out_ident();
        match value {
            Value::Static(json) => ops.lit(json),
            Value::Expr(expr) => ops.code(quote! { ::jt::ToJson::to_json(&(#expr), #out); }),
            Value::Object(entries) => {
                ops.lit("{");
                self.container_body(ops, entries);
                ops.lit("}");
            }
            Value::Array(elems) => {
                ops.lit("[");
                self.container_body(ops, elems);
                ops.lit("]");
            }
            Value::If(chain) => {
                let mut tokens = TokenStream::new();
                for (i, (cond, branch)) in chain.branches.iter().enumerate() {
                    let mut sub = Ops::default();
                    self.value(&mut sub, branch);
                    let sub = sub.into_tokens();
                    let kw = if i == 0 { quote!(if) } else { quote!(else if) };
                    tokens.extend(quote! { #kw #cond { #sub } });
                }
                let mut sub = Ops::default();
                match &chain.otherwise {
                    Some(v) => self.value(&mut sub, v),
                    None => sub.lit("null"),
                }
                let sub = sub.into_tokens();
                tokens.extend(quote! { else { #sub } });
                ops.code(tokens);
            }
            Value::Match(m) => {
                let scrutinee = &m.scrutinee;
                let mut arms = TokenStream::new();
                for (pat, v) in &m.arms {
                    let mut sub = Ops::default();
                    self.value(&mut sub, v);
                    let sub = sub.into_tokens();
                    arms.extend(quote! { #pat => { #sub } });
                }
                ops.code(quote! { match #scrutinee { #arms } });
            }
        }
    }

    /// Generates a container's contents with a fresh comma state, declaring
    /// the flag only if something used it.
    fn container_body<B: Body>(&mut self, ops: &mut Ops, body: &B) {
        let (ident, mut c) = self.container();
        let mark = ops.0.len();
        body.gen(self, ops, State::First, &mut c);
        if c.used {
            ops.0
                .insert(mark, Op::Quiet(quote! { let mut #ident = true; }));
        }
    }

    fn entries(
        &mut self,
        ops: &mut Ops,
        entries: &[Entry],
        mut state: State,
        c: &mut Container,
    ) -> State {
        let out = out_ident();
        for entry in entries {
            state = match entry {
                Entry::Field { key, value } => {
                    self.separator(ops, state, c);
                    match key {
                        Key::Static(k) => {
                            ops.lit(k);
                            ops.lit(":");
                        }
                        Key::Computed(k) => {
                            ops.code(quote! { ::jt::JsonKey::write_key(&(#k), #out); });
                            ops.lit(":");
                        }
                    }
                    self.value(ops, value);
                    State::Rest
                }
                Entry::Spread(expr) => {
                    self.sync(ops, state, c);
                    c.used = true;
                    let flag = &c.flag;
                    ops.code(quote! {
                        ::jt::JsonEntries::write_entries(&(#expr), #out, &mut #flag);
                    });
                    if state == State::Rest {
                        State::Rest
                    } else {
                        State::Unknown
                    }
                }
                Entry::If(chain) => self.if_chain(ops, chain, state, c),
                Entry::For(f) => self.for_loop(ops, f, state, c),
                Entry::Match(m) => self.match_arms(ops, m, state, c),
                Entry::Let(stmt) => {
                    ops.quiet(stmt.clone());
                    state
                }
            };
        }
        state
    }

    fn elems(
        &mut self,
        ops: &mut Ops,
        elems: &[Elem],
        mut state: State,
        c: &mut Container,
    ) -> State {
        for elem in elems {
            state = match elem {
                Elem::Value(v) => {
                    self.separator(ops, state, c);
                    self.value(ops, v);
                    State::Rest
                }
                Elem::If(chain) => self.if_chain(ops, chain, state, c),
                Elem::For(f) => self.for_loop(ops, f, state, c),
                Elem::Match(m) => self.match_arms(ops, m, state, c),
                Elem::Let(stmt) => {
                    ops.quiet(stmt.clone());
                    state
                }
            };
        }
        state
    }

    /// Generates each branch from the same entry state. If they end in
    /// different states, the result is `Unknown` and each branch that ended
    /// in a known state writes it to the flag.
    fn branches<B: Body>(
        &mut self,
        bodies: &[&B],
        has_default: bool,
        state: State,
        c: &mut Container,
    ) -> (Vec<Ops>, State) {
        let mut outs = Vec::with_capacity(bodies.len() + 1);
        for body in bodies {
            let mut sub = Ops::default();
            let end = body.gen(self, &mut sub, state, c);
            outs.push((sub, end));
        }
        if has_default {
            // The implicit `else` of an `if` with none: nothing is written.
            outs.push((Ops::default(), state));
        }
        let first = outs.first().map_or(state, |(_, end)| *end);
        let joined = if outs.iter().all(|(_, end)| *end == first) {
            first
        } else {
            State::Unknown
        };
        if joined == State::Unknown {
            for (sub, end) in &mut outs {
                self.sync(sub, *end, c);
            }
        }
        (outs.into_iter().map(|(sub, _)| sub).collect(), joined)
    }

    fn if_chain<B: Body>(
        &mut self,
        ops: &mut Ops,
        chain: &IfChain<B>,
        state: State,
        c: &mut Container,
    ) -> State {
        let mut bodies: Vec<&B> = chain.branches.iter().map(|(_, b)| b).collect();
        if let Some(otherwise) = &chain.otherwise {
            bodies.push(otherwise);
        }
        let (subs, joined) = self.branches(&bodies, chain.otherwise.is_none(), state, c);

        let mut tokens = TokenStream::new();
        let mut subs = subs.into_iter();
        for (i, (cond, _)) in chain.branches.iter().enumerate() {
            let sub = subs.next().unwrap().into_tokens();
            let kw = if i == 0 { quote!(if) } else { quote!(else if) };
            tokens.extend(quote! { #kw #cond { #sub } });
        }
        if let Some(sub) = subs.next() {
            if !sub.0.is_empty() {
                let sub = sub.into_tokens();
                tokens.extend(quote! { else { #sub } });
            }
        }
        ops.code(tokens);
        joined
    }

    fn match_arms<B: Body>(
        &mut self,
        ops: &mut Ops,
        m: &Match<B>,
        state: State,
        c: &mut Container,
    ) -> State {
        let bodies: Vec<&B> = m.arms.iter().map(|(_, b)| b).collect();
        let (subs, joined) = self.branches(&bodies, false, state, c);
        let scrutinee = &m.scrutinee;
        let arms = m.arms.iter().zip(subs).map(|((pat, _), sub)| {
            let sub = sub.into_tokens();
            quote! { #pat => { #sub } }
        });
        ops.code(quote! { match #scrutinee { #(#arms)* } });
        joined
    }

    fn for_loop<B: Body>(
        &mut self,
        ops: &mut Ops,
        f: &For<B>,
        state: State,
        c: &mut Container,
    ) -> State {
        // From `Rest`, every iteration is `Rest` too. Otherwise an iteration
        // cannot know whether an earlier one wrote anything.
        let entry = if state == State::Rest {
            State::Rest
        } else {
            self.sync(ops, state, c);
            State::Unknown
        };
        let mut sub = Ops::default();
        let end = f.body.gen(self, &mut sub, entry, c);
        if entry == State::Unknown {
            // The next iteration reads the flag.
            self.sync(&mut sub, end, c);
        }
        let out = out_ident();
        sub.code(quote! { ::jt::Out::checkpoint(#out); });
        let (pat, iter, sub) = (&f.pat, &f.iter, sub.into_tokens());
        ops.code(quote! { for #pat in #iter { #sub } });
        entry
    }
}

/// Generates the code for a `jt::json!` value, which evaluates to a `String`.
pub fn json(value: &Value) -> TokenStream {
    let mut ops = Ops::default();
    Gen::default().value(&mut ops, value);
    let capacity = (ops.static_len() + 64).next_power_of_two();
    let body = ops.into_tokens();
    let out = out_ident();
    quote! {{
        let mut __jt_buf = ::jt::__private::Buf::with_capacity(#capacity);
        {
            let #out = &mut __jt_buf;
            #body
        }
        __jt_buf.into_string()
    }}
}

/// Generates the code for `jt::write_json!(out, value)`.
pub fn write_json(target: &TokenStream, value: &Value) -> TokenStream {
    let mut ops = Ops::default();
    Gen::default().value(&mut ops, value);
    let body = ops.into_tokens();
    let out = out_ident();
    quote! {{
        let #out = &mut (#target);
        #body
    }}
}

/// Adds a lifetime to every reference in a parameter type that has none, so
/// the type can be a struct field.
struct Elide {
    lifetime: Lifetime,
    used: bool,
    error: Option<Error>,
}

impl VisitMut for Elide {
    fn visit_type_reference_mut(&mut self, r: &mut syn::TypeReference) {
        if r.lifetime.is_none() {
            r.lifetime = Some(self.lifetime.clone());
            self.used = true;
        }
        visit_mut::visit_type_reference_mut(self, r);
    }

    fn visit_lifetime_mut(&mut self, l: &mut Lifetime) {
        if l.ident == "_" {
            *l = self.lifetime.clone();
            self.used = true;
        }
    }

    fn visit_type_impl_trait_mut(&mut self, t: &mut syn::TypeImplTrait) {
        self.error.get_or_insert_with(|| {
            Error::new_spanned(
                &*t,
                "`impl Trait` parameters are not supported in templates; use a generic parameter",
            )
        });
    }

    // `fn(&str)` and `Fn(&str)` elide within themselves.
    fn visit_type_bare_fn_mut(&mut self, _: &mut syn::TypeBareFn) {}
    fn visit_parenthesized_generic_arguments_mut(
        &mut self,
        _: &mut syn::ParenthesizedGenericArguments,
    ) {
    }
}

/// Generates everything for one template.
pub fn template(t: &Template) -> Result<TokenStream> {
    let name = &t.sig.ident;
    let vis = &t.vis;
    let attrs = &t.attrs;
    let out = out_ident();

    if let Some(c) = &t.sig.constness {
        return Err(Error::new_spanned(c, "a template cannot be `const`"));
    }
    if let Some(a) = &t.sig.asyncness {
        return Err(Error::new_spanned(a, "a template cannot be `async`"));
    }

    let mut elide = Elide {
        lifetime: Lifetime::new("'jt", Span::call_site()),
        used: false,
        error: None,
    };
    let mut names = Vec::new();
    let mut field_types = Vec::new();
    for arg in &t.sig.inputs {
        match arg {
            FnArg::Receiver(r) => {
                return Err(Error::new_spanned(r, "a template cannot take `self`"));
            }
            FnArg::Typed(pt) => {
                let Pat::Ident(pi) = &*pt.pat else {
                    return Err(Error::new_spanned(
                        &pt.pat,
                        "template parameters must be plain names; destructure with `let` in the body",
                    ));
                };
                let mut ty: Type = (*pt.ty).clone();
                elide.visit_type_mut(&mut ty);
                names.push(pi.ident.clone());
                field_types.push(ty);
            }
        }
    }
    if let Some(e) = elide.error {
        return Err(e);
    }

    let mut generics = t.sig.generics.clone();
    if elide.used {
        generics.params.insert(
            0,
            GenericParam::Lifetime(LifetimeParam::new(elide.lifetime.clone())),
        );
    }
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let inputs = &t.sig.inputs;
    let hint = format_ident!("__JT_HINT_{}", name);

    let mut ops = Ops::default();
    let mut g = Gen::default();
    g.value(&mut ops, &t.body);
    let static_len = ops.static_len();
    let body = ops.into_tokens();

    let clones = names
        .iter()
        .map(|n| quote!(::core::clone::Clone::clone(&self.#n)));
    let clones2 = clones.clone();

    // A template whose body is an object can be spread into another object.
    let entries = if let Value::Object(entries) = &t.body {
        let mut ops = Ops::default();
        let mut c = Container {
            flag: quote!((*__jt_first)),
            used: false,
        };
        let end = g.entries(&mut ops, entries, State::Unknown, &mut c);
        g.sync(&mut ops, end, &mut c);
        let entries_body = ops.into_tokens();
        quote! {
            impl #impl_generics #name #ty_generics #where_clause {
                /// Writes this template's entries without the braces, for
                /// spreading into another object.
                #[allow(unused_mut, unused_variables, non_snake_case, clippy::all)]
                #vis fn write_entries<__JtO: ::jt::Out + ?Sized>(
                    #out: &mut __JtO,
                    __jt_first: &mut bool,
                    #inputs
                ) {
                    #entries_body
                }
            }

            impl #impl_generics ::jt::JsonEntries for #name #ty_generics #where_clause {
                #[inline]
                fn write_entries<__JtO: ::jt::Out + ?Sized>(&self, #out: &mut __JtO, __jt_first: &mut bool) {
                    Self::write_entries(#out, __jt_first, #(#clones2),*)
                }
            }
        }
    } else {
        TokenStream::new()
    };

    let doc = format!("Arguments of the `{name}` JSON template.");
    Ok(quote! {
        #(#attrs)*
        #[doc = #doc]
        #[derive(Clone)]
        #vis struct #name #impl_generics #where_clause {
            #(pub #names: #field_types,)*
        }

        /// Builds the template value. Render it with `jt::to_string` and friends.
        #[allow(non_snake_case, clippy::too_many_arguments)]
        #[inline]
        #vis fn #name #impl_generics (#(#names: #field_types),*) -> #name #ty_generics #where_clause {
            #name { #(#names),* }
        }

        #[allow(non_upper_case_globals)]
        static #hint: ::jt::__private::AtomicUsize = ::jt::__private::AtomicUsize::new(#static_len);

        impl #impl_generics #name #ty_generics #where_clause {
            /// Writes the template straight from its arguments, without
            /// building the struct.
            #[allow(unused_mut, unused_variables, non_snake_case, clippy::all)]
            #vis fn write<__JtO: ::jt::Out + ?Sized>(#out: &mut __JtO, #inputs) {
                #body
            }
        }

        impl #impl_generics ::jt::ToJson for #name #ty_generics #where_clause {
            #[inline]
            fn to_json<__JtO: ::jt::Out + ?Sized>(&self, #out: &mut __JtO) {
                Self::write(#out, #(#clones),*)
            }

            #[inline]
            fn json_size_hint(&self) -> usize {
                ::jt::__private::size_hint(&#hint)
            }

            #[inline]
            fn __jt_record_len(&self, len: usize) {
                ::jt::__private::record_len(&#hint, len)
            }
        }

        #entries
    })
}
