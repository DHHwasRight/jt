//! The grammars the crate exposes are the ones the editor ships.

fn same_as_editor(file: &str, crate_copy: &str) {
    let editor = format!(
        "{}/../../editors/vscode/syntaxes/{file}",
        env!("CARGO_MANIFEST_DIR")
    );
    // A published crate has no editors directory: there is nothing to compare against.
    let Ok(shipped) = std::fs::read_to_string(&editor) else {
        return;
    };
    assert!(
        shipped == crate_copy,
        "editors/vscode/syntaxes/{file} differs from crates/jt/grammar/{file}: edit the crate copy, then copy it over the editor one"
    );
}

#[test]
fn the_crate_grammars_are_the_ones_the_vscode_extension_ships() {
    same_as_editor("jt.tmLanguage.json", jt::grammar::TEXTMATE);
    same_as_editor("jt-injection.tmLanguage.json", jt::grammar::INJECTION);
}

#[test]
fn the_grammars_name_their_scopes() {
    assert!(
        jt::grammar::TEXTMATE.contains(&format!("\"scopeName\": \"{}\"", jt::grammar::SCOPE_NAME))
    );
    assert!(jt::grammar::INJECTION.contains(&format!(
        "\"scopeName\": \"{}\"",
        jt::grammar::INJECTION_SCOPE_NAME
    )));
    assert!(
        jt::grammar::INJECTION.contains("\"source.jt\""),
        "the injection builds on the .jt grammar"
    );
}
