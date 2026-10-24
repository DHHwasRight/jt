//! Prints the Rust generated for a template file: `cargo run -p jt-core --example expand -- file.jt`
use std::str::FromStr;

fn main() {
    let path = std::env::args().nth(1).expect("usage: expand <file.jt>");
    let source = std::fs::read_to_string(&path).expect("cannot read file");
    let tokens = proc_macro2::TokenStream::from_str(&source).expect("cannot tokenize");
    match jt_core::expand_items(tokens) {
        Ok(out) => println!("{out}"),
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    }
}
