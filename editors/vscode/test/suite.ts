// Runs inside VS Code: drives the extension through VS Code's own APIs.
import * as assert from "assert";
import * as path from "path";
import * as vscode from "vscode";

const fixture = path.resolve(__dirname, "..", "test", "fixture");

async function until<T>(what: string, probe: () => Promise<T | undefined> | T | undefined): Promise<T> {
  const deadline = Date.now() + 20000;
  for (;;) {
    const value = await probe();
    if (value !== undefined) {
      return value;
    }
    if (Date.now() > deadline) {
      throw new Error(`timed out waiting for ${what}`);
    }
    await new Promise((r) => setTimeout(r, 100));
  }
}

function at(doc: vscode.TextDocument, needle: string, offset = 1): vscode.Position {
  const i = doc.getText().indexOf(needle);
  assert.ok(i >= 0, `${needle} not in ${doc.uri.fsPath}`);
  return doc.positionAt(i + offset);
}

async function check(name: string, body: () => Promise<void>) {
  await body();
  console.log(`  ok  ${name}`);
}

export async function run() {
  const user = await vscode.workspace.openTextDocument(path.join(fixture, "user.jt"));
  const editor = await vscode.window.showTextDocument(user);

  await check("the .jt language is registered", async () => {
    assert.strictEqual(user.languageId, "jt");
  });

  await check("hover shows the signature, docs and keys of a template in another file", async () => {
    const text = await until("hover", async () => {
      const hovers = await vscode.commands.executeCommand<vscode.Hover[]>(
        "vscode.executeHoverProvider", user.uri, at(user, "Base(id)"));
      const parts = hovers.flatMap((h) => h.contents.map((c) => (typeof c === "string" ? c : c.value)));
      return parts.length ? parts.join("\n") : undefined;
    });
    assert.ok(text.includes("pub fn Base(id: u64) -> Json"), text);
    assert.ok(text.includes("Fields every record has."), text);
    assert.ok(text.includes("`id`, `kind`"), text);
  });

  await check("go to definition crosses files", async () => {
    const defs = await vscode.commands.executeCommand<(vscode.Location | vscode.LocationLink)[]>(
      "vscode.executeDefinitionProvider", user.uri, at(user, "Base(id)"));
    assert.strictEqual(defs.length, 1);
    const target = "targetUri" in defs[0] ? defs[0].targetUri : defs[0].uri;
    assert.strictEqual(path.basename(target.fsPath), "base.jt");
  });

  await check("outline lists the template", async () => {
    const symbols = await vscode.commands.executeCommand<vscode.DocumentSymbol[]>(
      "vscode.executeDocumentSymbolProvider", user.uri);
    assert.deepStrictEqual(symbols.map((s) => s.name), ["User"]);
  });

  await check("completion offers templates from the workspace and snippets", async () => {
    const list = await vscode.commands.executeCommand<vscode.CompletionList>(
      "vscode.executeCompletionItemProvider", user.uri, at(user, "\"name\"", 0));
    const labels = list.items.map((i) => (typeof i.label === "string" ? i.label : i.label.label));
    for (const want of ["Base", "User", "for", "match"]) {
      assert.ok(labels.includes(want), `${want} missing from ${labels.join(", ")}`);
    }
  });

  await check("references find the use and the definition", async () => {
    const refs = await vscode.commands.executeCommand<vscode.Location[]>(
      "vscode.executeReferenceProvider", user.uri, at(user, "Base(id)"));
    const files = refs.map((r) => path.basename(r.uri.fsPath)).sort();
    assert.deepStrictEqual(files, ["base.jt", "user.jt"]);
  });

  await check("an edit that breaks the file shows an error at the token", async () => {
    const name = user.getText().indexOf("\"name\":");
    await editor.edit((e) => e.delete(new vscode.Range(user.positionAt(name + 6), user.positionAt(name + 7))));
    const diags = await until("diagnostics", () => {
      const d = vscode.languages.getDiagnostics(user.uri);
      return d.length ? d : undefined;
    });
    assert.strictEqual(diags.length, 1);
    assert.strictEqual(diags[0].message, "expected `:` after the key");
    assert.strictEqual(diags[0].source, "jt");
    assert.strictEqual(diags[0].range.start.line, 2);
    await vscode.commands.executeCommand("undo");
    await until("diagnostics to clear", () =>
      vscode.languages.getDiagnostics(user.uri).length === 0 ? true : undefined);
  });

  await check("Show Generated Rust opens the expansion beside the file", async () => {
    await vscode.window.showTextDocument(user);
    await vscode.commands.executeCommand("jt.showExpansion");
    const expansion = await until("the expansion editor", () =>
      vscode.window.visibleTextEditors.find((e) => e.document.uri.scheme === "jt-expansion"));
    assert.strictEqual(expansion.document.languageId, "rust");
    const code = expansion.document.getText();
    assert.ok(code.includes("impl<'jt> User<'jt>"), code);
    assert.ok(code.includes("Base"), code);
  });
}
