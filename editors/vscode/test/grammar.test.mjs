// The TextMate grammars, run the way VS Code runs them: vscode-textmate with the Oniguruma engine,
// and the Rust grammar VS Code ships (from `tm-grammars`).
//
// Checks scopes at chosen places in a `.jt` sample and in a Rust file that uses jt's macros, then
// that every `.jt` file in this repository has its keys scoped as keys.

import assert from "node:assert";
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import vsctm from "vscode-textmate";
import oniguruma from "vscode-oniguruma";

const require = createRequire(import.meta.url);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repo = path.resolve(root, "../..");
const read = (p) => JSON.parse(fs.readFileSync(p, "utf8"));

await oniguruma.loadWASM(fs.readFileSync(require.resolve("vscode-oniguruma/release/onig.wasm")).buffer);
const byScope = {
  "source.jt": read(path.join(root, "syntaxes/jt.tmLanguage.json")),
  "source.rust.jt-injection": read(path.join(root, "syntaxes/jt-injection.tmLanguage.json")),
  "source.rust": read(require.resolve("tm-grammars/grammars/rust.json")),
};
const registry = new vsctm.Registry({
  onigLib: Promise.resolve({
    createOnigScanner: (s) => new oniguruma.OnigScanner(s),
    createOnigString: (s) => new oniguruma.OnigString(s),
  }),
  loadGrammar: async (scope) => byScope[scope] ?? null,
  // What `injectTo` in package.json does in VS Code.
  getInjections: (scope) => (scope === "source.rust" ? ["source.rust.jt-injection"] : undefined),
});

function tokenize(grammar, source) {
  let stack = vsctm.INITIAL;
  const out = [];
  source.split("\n").forEach((line, i) => {
    const r = grammar.tokenizeLine(line, stack);
    stack = r.ruleStack;
    for (const t of r.tokens) out.push({ line: i + 1, text: line.slice(t.startIndex, t.endIndex), scopes: t.scopes.join(" ") });
  });
  return out;
}

function expectScopes(tokens, expectations) {
  for (const [line, text, scope, not] of expectations) {
    const t = tokens.find((t) => t.line === line && t.text.trim() === text);
    assert.ok(t, `no token "${text}" on line ${line}`);
    if (not) assert.ok(!t.scopes.includes(scope), `line ${line} "${text}": unexpected ${scope} in ${t.scopes}`);
    else assert.ok(t.scopes.includes(scope), `line ${line} "${text}": expected ${scope}, got ${t.scopes}`);
  }
}

const JT = `pub struct Account {
    id: u64,
}

pub fn Page(users: &[Account], next: Option<&str>) -> Json {
    "data": [ for u in users { User(u) } ],
    "next"?: next,
    r#type: 1,
    count?: users.len(),
    ...Base(7),
    "missing": null,
}

pub fn Ids(ids: &[u64]) -> Json = [ ...ids ];
`;

const RS = `fn render(id: u64) -> String {
    jt::json!({ "id": id, name: "x", "f": f(1, (2)) })
}

jt::template! {
    pub fn User(u: &Account) -> Json {
        "id": u.id,
        "email"?: u.email,
    }
}

const NOT_A_KEY: &str = "id";
`;

const jt = await registry.loadGrammar("source.jt");
expectScopes(tokenize(jt, JT), [
  // Struct fields are Rust, not keys.
  [2, "id", "support.type.property-name.json", true],
  [5, "Page", "entity.name.function.jt"],
  [5, "Json", "support.type.jt"],
  [6, "data", "support.type.property-name.json"],
  [6, "for", "keyword.control.rust"],
  [7, "?", "keyword.operator.optional.jt"],
  [8, "r#type", "support.type.property-name.json"],
  [9, "count", "support.type.property-name.json"],
  [10, "...", "keyword.operator.spread.jt"],
  [11, "null", "constant.language.null.jt"],
  [14, "...", "keyword.operator.spread.jt"],
]);

const rust = await registry.loadGrammar("source.rust");
expectScopes(tokenize(rust, RS), [
  [2, "id", "support.type.property-name.json"],
  [2, "name", "support.type.property-name.json"],
  [2, "f", "support.type.property-name.json"],
  [7, "id", "support.type.property-name.json"],
  [8, "?", "keyword.operator.optional.jt"],
  // After the macros, Rust is Rust again.
  [12, "id", "string.quoted.double.rust"],
]);

// Every .jt file in the repository: at least one key, and every `"…":` at the start of a line is a key.
const files = [];
const walk = (dir) => {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    if (["target", "node_modules", ".git", ".vscode-test", "out"].includes(e.name)) continue;
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walk(p);
    else if (e.name.endsWith(".jt")) files.push(p);
  }
};
walk(repo);
const problems = [];
for (const file of files) {
  const tokens = tokenize(jt, fs.readFileSync(file, "utf8"));
  const keyLines = new Set(tokens.filter((t) => t.scopes.includes("support.type.property-name.json")).map((t) => t.line));
  fs.readFileSync(file, "utf8")
    .split("\n")
    .forEach((line, i) => {
      if (/^\s*"[^"]*"\??:(?!:)/.test(line) && !keyLines.has(i + 1)) problems.push(`${path.relative(repo, file)}:${i + 1}: ${line.trim()}`);
    });
}
assert.ok(files.length > 0, "no .jt files found");
assert.deepStrictEqual(problems, [], `keys not scoped as keys:\n${problems.join("\n")}`);
console.log(`grammar: .jt and Rust injection scopes ok, ${files.length} .jt files ok`);
