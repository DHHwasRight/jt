# jt for VS Code

Language support for jt `.jt` template files.

- Highlighting: Rust, plus JSON keys, `?:`, `...` and `null`.
- Errors from the jt parser while you type, one per broken item.
- Go to definition, find references and hover for templates across the
  workspace. Hover shows the signature, the docs and the object keys.
- Completion of template names and template syntax (`for`, `if`, `match`,
  `let`, `"key"?:`).
- Outline and workspace symbols (`Cmd+T`).
- **jt: Show Generated Rust**: the code that `jt::load!` makes from the file,
  beside the source. It updates as you type.

## Install

From the repository root:

```sh
mise run vscode
```

This builds `jt-lsp` in release mode, puts it inside the extension, packages
`jt.vsix` and installs it with `code --install-extension`.

## Server lookup

1. The `jt.server.path` setting.
2. The `jt-lsp` binary bundled in the extension.
3. `jt-lsp` on `PATH`, then `~/.cargo/bin/jt-lsp` (`mise run lsp`).

## Limits

- Types are not checked. Expressions are parsed as Rust, but a type error
  appears only when cargo builds the crate that loads the file.
- Definition and references match names, not module paths. Two templates
  with the same name in different files both match.
- Files for the Go or Crystal jt (they start with `package` or `module`)
  get highlighting but no diagnostics.
