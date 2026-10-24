//! Runs the server binary and speaks LSP to it over stdin and stdout.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    /// Notifications that arrived while waiting for a response.
    notifications: Vec<Value>,
}

impl Client {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_jt-lsp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start jt-lsp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Client {
            child,
            stdin,
            stdout,
            next_id: 1,
            notifications: Vec::new(),
        }
    }

    fn send(&mut self, message: Value) {
        let body = message.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn receive(&mut self) -> Value {
        let mut length = 0;
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(n) = line.strip_prefix("Content-Length: ") {
                length = n.parse().unwrap();
            }
        }
        let mut body = vec![0; length];
        self.stdout.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let message = self.receive();
            if message["id"] == id {
                assert!(message.get("error").is_none(), "{method}: {message}");
                return message["result"].clone();
            }
            self.notifications.push(message);
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// The latest diagnostics published for `uri`, waiting if none came yet.
    fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        loop {
            let found = self.notifications.iter().rev().find(|n| {
                n["method"] == "textDocument/publishDiagnostics" && n["params"]["uri"] == uri
            });
            if let Some(n) = found {
                let d = n["params"]["diagnostics"].as_array().unwrap().clone();
                self.notifications.clear();
                return d;
            }
            let message = self.receive();
            self.notifications.push(message);
        }
    }
}

fn views() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../jt/tests/views")
        .canonicalize()
        .unwrap()
}

fn uri(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().to_string()
}

#[test]
fn a_session() {
    let root = views();
    let user = root.join("user.jt");
    let user_uri = uri(&user);
    let text = std::fs::read_to_string(&user).unwrap();

    let mut c = Client::start();
    let init = c.request(
        "initialize",
        json!({ "capabilities": {}, "rootUri": uri(&root), "workspaceFolders": null }),
    );
    assert_eq!(init["serverInfo"]["name"], "jt-lsp");
    c.notify("initialized", json!({}));
    // The scan publishes diagnostics for every file in the workspace.
    assert_eq!(c.diagnostics(&user_uri), Vec::<Value>::new());

    c.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": user_uri, "languageId": "jt", "version": 1, "text": text } }),
    );
    assert_eq!(c.diagnostics(&user_uri), Vec::<Value>::new());

    let line = text
        .lines()
        .position(|l| l.contains("pub fn User"))
        .unwrap();
    let character = text.lines().nth(line).unwrap().find("User").unwrap() + 1;
    let at = json!({ "textDocument": { "uri": user_uri }, "position": { "line": line, "character": character } });

    let hover = c.request("textDocument/hover", at.clone());
    let hover = hover["contents"]["value"].as_str().unwrap();
    assert!(
        hover.contains("pub fn User(account: &Account<'_>) -> Json"),
        "{hover}"
    );
    assert!(hover.contains("`email?`"), "{hover}");

    let def = c.request("textDocument/definition", at.clone());
    assert_eq!(def[0]["uri"], user_uri);
    assert_eq!(def[0]["range"]["start"]["line"], line);

    let symbols = c.request(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": user_uri } }),
    );
    let names: Vec<&str> = symbols
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Account", "initials", "User"]);

    let all = c.request("workspace/symbol", json!({ "query": "page" }));
    assert!(
        all.as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "CreatePage"),
        "{all}"
    );

    let expanded = c.request("jt/expand", json!({ "textDocument": { "uri": user_uri } }));
    let code = expanded["code"].as_str().unwrap();
    assert!(code.contains("impl<'jt> User<'jt>"), "{code}");

    let completion = c.request("textDocument/completion", at.clone());
    let labels: Vec<&str> = completion
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect();
    assert!(
        labels.contains(&"User") && labels.contains(&"CreatePage") && labels.contains(&"for"),
        "{labels:?}"
    );

    // An edit that breaks the file is reported where it breaks.
    let broken = text.replace("\"id\": account.id,", "\"id\" account.id,");
    c.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": user_uri, "version": 2 }, "contentChanges": [{ "text": broken }] }),
    );
    let d = c.diagnostics(&user_uri);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0]["message"], "expected `:` after the key");
    let bad_line = broken
        .lines()
        .position(|l| l.contains("\"id\" account"))
        .unwrap();
    assert_eq!(d[0]["range"]["start"]["line"], bad_line);

    // Closing the file goes back to the clean version on disk.
    c.notify(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": user_uri } }),
    );
    assert_eq!(c.diagnostics(&user_uri), Vec::<Value>::new());

    c.request("shutdown", Value::Null);
    c.notify("exit", Value::Null);
    assert!(c.child.wait().unwrap().success());
}
