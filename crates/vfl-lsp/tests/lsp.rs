//! The server over an in-memory connection, driven like an editor would.

use std::path::Path;
use std::thread;

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use serde_json::{Value, json};

struct Client {
    connection: Connection,
    server: Option<thread::JoinHandle<()>>,
    next_id: i32,
    /// Notifications received while waiting for responses.
    notifications: Vec<Notification>,
}

impl Client {
    fn start(root: &Path) -> Client {
        let (server, connection) = Connection::memory();
        let handle = thread::spawn(move || vfl_lsp::run(&server).unwrap());
        let mut client = Client {
            connection,
            server: Some(handle),
            next_id: 0,
            notifications: Vec::new(),
        };
        let root = url(root);
        client.request(
            "initialize",
            json!({ "capabilities": {}, "rootUri": root, "processId": null }),
        );
        client.notify("initialized", json!({}));
        client
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.connection
            .sender
            .send(Message::Request(Request::new(
                id.clone(),
                method.into(),
                params,
            )))
            .unwrap();
        loop {
            match self.connection.receiver.recv().unwrap() {
                Message::Response(r) if r.id == id => {
                    assert!(r.error.is_none(), "{method}: {:?}", r.error);
                    return r.result.unwrap_or(Value::Null);
                }
                Message::Notification(n) => self.notifications.push(n),
                _ => {}
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.connection
            .sender
            .send(Message::Notification(Notification::new(
                method.into(),
                params,
            )))
            .unwrap();
    }

    /// The latest diagnostics published for `uri`.
    fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        // A request round-trip makes sure earlier notifications arrived.
        self.request("workspace/symbol", json!({ "query": "" }));
        self.notifications
            .iter()
            .rev()
            .find(|n| n.method == "textDocument/publishDiagnostics" && n.params["uri"] == uri)
            .map(|n| n.params["diagnostics"].as_array().unwrap().clone())
            .unwrap_or_default()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        self.server.take().unwrap().join().unwrap();
    }
}

fn url(path: &Path) -> String {
    lsp_types::Url::from_file_path(path).unwrap().to_string()
}

const PKG: &str = "package pkg is\n  constant width : natural := 8;\nend package;\n";
const TOP: &str = "use work.pkg.all;\n\
entity top is\n  port (d : out bit_vector(width - 1 downto 0));\nend entity top;\n\
architecture rtl of top is\nbegin\n  d <= (others => '0');\nend architecture;\n";

fn setup() -> (tempfile::TempDir, String, String) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vfl.toml"), "").unwrap();
    std::fs::write(dir.path().join("pkg.vhd"), PKG).unwrap();
    std::fs::write(dir.path().join("top.vhd"), TOP).unwrap();
    let pkg = url(&dir.path().join("pkg.vhd"));
    let top = url(&dir.path().join("top.vhd"));
    (dir, pkg, top)
}

fn at(uri: &str, line: u32, character: u32) -> Value {
    json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } })
}

#[test]
fn navigation_across_files() {
    let (dir, pkg, top) = setup();
    let mut client = Client::start(dir.path());

    // `width` in top.vhd, line 2.
    let definition = client.request("textDocument/definition", at(&top, 2, 29));
    assert_eq!(definition[0]["uri"], pkg);
    assert_eq!(
        definition[0]["range"]["start"],
        json!({ "line": 1, "character": 11 })
    );

    let hover = client.request("textDocument/hover", at(&top, 2, 29));
    let text = hover["contents"]["value"].as_str().unwrap();
    assert!(text.starts_with("constant `width` in `work.pkg`"), "{text}");
    assert!(text.contains("constant width : natural := 8;"), "{text}");

    let mut references = at(&pkg, 1, 13);
    references["context"] = json!({ "includeDeclaration": true });
    let references = client.request("textDocument/references", references);
    assert_eq!(references.as_array().unwrap().len(), 2, "{references}");

    let mut rename = at(&pkg, 1, 13);
    rename["newName"] = json!("data_width");
    let edit = client.request("textDocument/rename", rename);
    assert_eq!(edit["changes"][&pkg].as_array().unwrap().len(), 1);
    assert_eq!(edit["changes"][&top][0]["newText"], "data_width");

    // The entity name and its end label.
    let mut rename = at(&top, 1, 8);
    rename["newName"] = json!("chip");
    let edit = client.request("textDocument/rename", rename);
    assert_eq!(edit["changes"][&top].as_array().unwrap().len(), 3, "{edit}");

    let symbols = client.request(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": top } }),
    );
    assert_eq!(symbols[0]["name"], "top");
    assert_eq!(symbols[0]["children"][0]["name"], "d");
    let symbols = client.request("workspace/symbol", json!({ "query": "wid" }));
    assert_eq!(symbols[0]["name"], "width");

    let completion = client.request("textDocument/completion", at(&top, 6, 2));
    let labels: Vec<&str> = completion
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect();
    for wanted in ["d", "width", "natural", "top", "process"] {
        assert!(labels.contains(&wanted), "{wanted} missing from {labels:?}");
    }
}

#[test]
fn edits_update_diagnostics_and_fixes() {
    let (dir, _, top) = setup();
    std::fs::write(
        dir.path().join("vfl.toml"),
        "[lint.rules]\n\"style/trailing-whitespace\" = \"warn\"\n",
    )
    .unwrap();
    let mut client = Client::start(dir.path());
    assert!(client.diagnostics(&top).is_empty());

    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": top, "languageId": "vhdl", "version": 1, "text": TOP } }),
    );
    // Replace `'0'` with a name that does not exist, and add trailing spaces.
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": top, "version": 2 },
            "contentChanges": [
                { "range": { "start": { "line": 6, "character": 18 }, "end": { "line": 6, "character": 21 } }, "text": "nope" },
                { "range": { "start": { "line": 5, "character": 5 }, "end": { "line": 5, "character": 5 } }, "text": "  " }
            ]
        }),
    );
    let diagnostics = client.diagnostics(&top);
    let codes: Vec<&str> = diagnostics
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert_eq!(
        codes,
        ["style/trailing-whitespace", "sema/unresolved-name"],
        "{diagnostics:?}"
    );

    let actions = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": top },
            "range": { "start": { "line": 5, "character": 0 }, "end": { "line": 5, "character": 7 } },
            "context": { "diagnostics": [] }
        }),
    );
    assert_eq!(actions[0]["title"], "Remove trailing whitespace");
    assert_eq!(
        actions[0]["edit"]["changes"][&top][0]["range"]["start"],
        json!({ "line": 5, "character": 5 })
    );

    let formatting = client.request(
        "textDocument/formatting",
        json!({ "textDocument": { "uri": top }, "options": { "tabSize": 2, "insertSpaces": true } }),
    );
    assert_eq!(formatting.as_array().unwrap().len(), 1, "{formatting}");

    // Closing the document goes back to the file on disk.
    client.notify(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": top } }),
    );
    assert!(client.diagnostics(&top).is_empty());
}
