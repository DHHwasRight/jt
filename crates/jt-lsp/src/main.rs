//! A language server for jt `.jt` template files.
//!
//! It speaks LSP over stdin and stdout and gives:
//!
//! - diagnostics from the jt parser and code generator, one per broken item,
//! - document and workspace symbols for templates and Rust items,
//! - hover with a template's signature, docs and object keys,
//! - go to definition and find references across every `.jt` file in the
//!   workspace,
//! - completion of template names and template syntax,
//! - the `jt/expand` request, which returns the generated Rust code.
//!
//! The expressions inside a template are Rust, but no type checker runs
//! here: types are checked when cargo builds the crate that loads the file.

mod analysis;
mod line_index;
mod server;

use std::error::Error;

fn main() -> Result<(), Box<dyn Error + Sync + Send>> {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("jt-lsp {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let (connection, io_threads) = lsp_server::Connection::stdio();
    server::run(&connection)?;
    drop(connection);
    io_threads.join()?;
    Ok(())
}
