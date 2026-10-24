// VS Code client for the jt language server.
//
// The server does the work: diagnostics, symbols, hover, definition,
// references and completion. This file finds the server binary, starts it,
// and adds the "Show Generated Rust" view.

import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

const EXPANSION_SCHEME = "jt-expansion";

let client: LanguageClient | undefined;

interface ExpandResult {
  code: string | null;
  error: string | null;
}

export async function activate(context: vscode.ExtensionContext) {
  const expansions = new ExpansionProvider();
  context.subscriptions.push(
    vscode.workspace.registerTextDocumentContentProvider(EXPANSION_SCHEME, expansions),
    vscode.commands.registerCommand("jt.showExpansion", () => showExpansion(expansions)),
    vscode.commands.registerCommand("jt.restartServer", async () => {
      await stop();
      await start(context);
    }),
    vscode.workspace.onDidChangeConfiguration(async (e) => {
      if (e.affectsConfiguration("jt.server.path")) {
        await stop();
        await start(context);
      }
    }),
    // Keep an open expansion in step with its source.
    vscode.workspace.onDidChangeTextDocument((e) => {
      if (e.document.languageId === "jt") {
        expansions.refresh(e.document.uri);
      }
    }),
  );
  await start(context);
}

export function deactivate(): Thenable<void> | undefined {
  return stop();
}

async function start(context: vscode.ExtensionContext) {
  const command = findServer(context);
  if (!command) {
    const choice = await vscode.window.showErrorMessage(
      "jt: cannot find the jt-lsp binary. Install it with `cargo install --path crates/jt-lsp`, or set `jt.server.path`.",
      "Open Settings",
    );
    if (choice) {
      vscode.commands.executeCommand("workbench.action.openSettings", "jt.server.path");
    }
    return;
  }
  const serverOptions: ServerOptions = { command, args: [] };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "jt" }],
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher("**/*.jt"),
    },
  };
  client = new LanguageClient("jt", "jt language server", serverOptions, clientOptions);
  await client.start();
}

function stop(): Thenable<void> | undefined {
  const running = client;
  client = undefined;
  return running?.stop();
}

/** The setting, then the bundled binary, then `jt-lsp` on PATH. */
function findServer(context: vscode.ExtensionContext): string | undefined {
  const configured = vscode.workspace.getConfiguration("jt").get<string>("server.path");
  if (configured) {
    return configured.replace(/^~(?=$|\/)/, process.env.HOME ?? "~");
  }
  const exe = process.platform === "win32" ? "jt-lsp.exe" : "jt-lsp";
  const bundled = context.asAbsolutePath(path.join("server", exe));
  if (fs.existsSync(bundled)) {
    return bundled;
  }
  const dirs = (process.env.PATH ?? "").split(path.delimiter);
  // Shells add cargo's bin directory to PATH, but VS Code started from the
  // dock may not have it.
  dirs.push(path.join(process.env.HOME ?? "", ".cargo", "bin"));
  for (const dir of dirs) {
    const candidate = path.join(dir, exe);
    if (dir && fs.existsSync(candidate)) {
      return candidate;
    }
  }
  return undefined;
}

/** Serves `jt-expansion:` documents: the Rust code generated for a `.jt` file. */
class ExpansionProvider implements vscode.TextDocumentContentProvider {
  private changes = new vscode.EventEmitter<vscode.Uri>();
  readonly onDidChange = this.changes.event;

  static uriFor(source: vscode.Uri): vscode.Uri {
    return vscode.Uri.from({
      scheme: EXPANSION_SCHEME,
      path: source.path + ".rs",
      query: source.toString(),
    });
  }

  refresh(source: vscode.Uri) {
    const uri = ExpansionProvider.uriFor(source);
    if (vscode.workspace.textDocuments.some((d) => d.uri.toString() === uri.toString())) {
      this.changes.fire(uri);
    }
  }

  async provideTextDocumentContent(uri: vscode.Uri): Promise<string> {
    if (!client) {
      return "// The jt language server is not running.";
    }
    const result = await client.sendRequest<ExpandResult>("jt/expand", {
      textDocument: { uri: uri.query },
    });
    if (result.error) {
      return `// jt cannot expand this file:\n// ${result.error.replace(/\n/g, "\n// ")}\n`;
    }
    return result.code ?? "";
  }
}

async function showExpansion(expansions: ExpansionProvider) {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== "jt") {
    vscode.window.showInformationMessage("jt: open a .jt file first.");
    return;
  }
  const uri = ExpansionProvider.uriFor(editor.document.uri);
  expansions.refresh(editor.document.uri);
  const doc = await vscode.workspace.openTextDocument(uri);
  await vscode.languages.setTextDocumentLanguage(doc, "rust");
  await vscode.window.showTextDocument(doc, {
    viewColumn: vscode.ViewColumn.Beside,
    preserveFocus: true,
    preview: true,
  });
}
