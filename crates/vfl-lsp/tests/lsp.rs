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

const TYPES_PKG: &str = "\
package types is
  type pair_t is record
    lo : integer;
    hi : bit_vector(7 downto 0);
  end record;
  type outer_t is record
    inner : pair_t;
    flag : boolean;
  end record;
  type outer_array is array (natural range <>) of outer_t;
  subtype byte is bit_vector(7 downto 0);
  type counter_t is protected
    procedure add(n : integer);
    impure function get return integer;
  end protected;
  function scale(x : integer; factor : integer := 2) return integer;
  function scale(x : real) return real;
end package;
package body types is
  type counter_t is protected body
    variable count : integer := 0;
    procedure add(n : integer) is begin count := count + n; end procedure;
    impure function get return integer is begin return count; end function;
  end protected body;
  function scale(x : integer; factor : integer := 2) return integer is
  begin return x * factor; end function;
  function scale(x : real) return real is begin return x * 2.0; end function;
end package body;
";

const TYPES_USE: &str = "\
use work.types.all;
entity leaf is
  port (a : in bit; y : out bit);
end entity;
architecture rtl of leaf is begin y <= a; end architecture;
use work.types.all;
entity user is end entity;
architecture rtl of user is
  signal o : outer_t;
  signal list : outer_array(0 to 3);
  signal b : byte;
  shared variable c : counter_t;
  signal s1, s2 : bit;
begin
  process
    variable v : integer;
  begin
    v := o.inner.lo + list(0).inner.lo;
    v := scale(v, 3);
    c.add(1);
    wait;
  end process;
  u : entity work.leaf port map (a => s1, y => s2);
end architecture;
";

#[test]
fn range_and_on_type_formatting() {
    let (dir, _, top) = setup();
    let mut client = Client::start(dir.path());
    let text = "use work.pkg.all;\nentity top is\nend entity top;\narchitecture rtl of top is\n\
        signal a,b:bit;\nbegin\na<=b;\n\n    b   <=   a;\nend architecture;\n";
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": top, "languageId": "vhdl", "version": 1, "text": text } }),
    );
    let edits = client.request(
        "textDocument/rangeFormatting",
        json!({
            "textDocument": { "uri": top },
            "range": { "start": { "line": 8, "character": 0 }, "end": { "line": 8, "character": 3 } },
            "options": { "tabSize": 2, "insertSpaces": true }
        }),
    );
    assert_eq!(
        edits,
        json!([{
            "range": { "start": { "line": 8, "character": 0 }, "end": { "line": 8, "character": 15 } },
            "newText": "  b <= a;"
        }])
    );

    // A `;` just typed formats its line.
    let typed = client.request(
        "textDocument/onTypeFormatting",
        json!({
            "textDocument": { "uri": top },
            "position": { "line": 6, "character": 5 },
            "ch": ";",
            "options": { "tabSize": 2, "insertSpaces": true }
        }),
    );
    assert_eq!(typed[0]["newText"], "  a <= b;", "{typed}");

    // A new line formats the line above it and leaves the new one alone.
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": top, "version": 2 },
            "contentChanges": [
                { "range": { "start": { "line": 4, "character": 15 }, "end": { "line": 4, "character": 15 } }, "text": "\n  " }
            ]
        }),
    );
    let typed = client.request(
        "textDocument/onTypeFormatting",
        json!({
            "textDocument": { "uri": top },
            "position": { "line": 5, "character": 2 },
            "ch": "\n",
            "options": { "tabSize": 2, "insertSpaces": true }
        }),
    );
    assert_eq!(
        typed,
        json!([{
            "range": { "start": { "line": 4, "character": 0 }, "end": { "line": 4, "character": 15 } },
            "newText": "  signal a, b : bit;"
        }])
    );
}

fn types_setup() -> (tempfile::TempDir, Client, String) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vfl.toml"), "").unwrap();
    std::fs::write(dir.path().join("types.vhd"), TYPES_PKG).unwrap();
    std::fs::write(dir.path().join("user.vhd"), TYPES_USE).unwrap();
    let client = Client::start(dir.path());
    let user = url(&dir.path().join("user.vhd"));
    (dir, client, user)
}

fn hover_text(client: &mut Client, uri: &str, line: u32, character: u32) -> String {
    let hover = client.request("textDocument/hover", at(uri, line, character));
    hover["contents"]["value"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn labels(completion: &Value) -> Vec<String> {
    completion
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn hover_shows_types() {
    let (_dir, mut client, user) = types_setup();
    // The signal `o`, its record type and elements.
    let text = hover_text(&mut client, &user, 17, 10);
    assert!(text.contains("type `outer_t`"), "{text}");
    assert!(text.contains("`inner` : `pair_t`"), "{text}");
    // A subtype shows its base type.
    let text = hover_text(&mut client, &user, 10, 9);
    assert!(text.contains("`byte` (subtype of `bit_vector`)"), "{text}");
    // Record elements, through an element and an indexed array.
    let text = hover_text(&mut client, &user, 17, 17);
    assert!(text.contains("element `lo` of `pair_t`"), "{text}");
    assert!(text.contains("type `integer`"), "{text}");
    let text = hover_text(&mut client, &user, 17, 31);
    assert!(text.contains("element `inner` of `outer_t`"), "{text}");
    // The overload a call resolves to.
    let text = hover_text(&mut client, &user, 18, 10);
    assert!(text.contains("returns `integer`"), "{text}");
    assert!(!text.contains("real"), "{text}");
}

#[test]
fn completion_of_elements_and_methods() {
    let (_dir, mut client, user) = types_setup();
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": user, "languageId": "vhdl", "version": 1, "text": TYPES_USE } }),
    );
    let complete = |client: &mut Client, version: i32, text: &str, character: u32| {
        client.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": user, "version": version },
                "contentChanges": [
                    { "range": { "start": { "line": 20, "character": 4 }, "end": { "line": 20, "character": 9 } }, "text": text }
                ]
            }),
        );
        let completion = client.request("textDocument/completion", at(&user, 20, character));
        // Undo the edit.
        client.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": user, "version": version + 1 },
                "contentChanges": [
                    { "range": { "start": { "line": 20, "character": 4 }, "end": { "line": 20, "character": 4 + text.len() as u32 } }, "text": "wait;" }
                ]
            }),
        );
        labels(&completion)
    };
    assert_eq!(complete(&mut client, 2, "o.", 6), ["inner", "flag"]);
    assert_eq!(complete(&mut client, 4, "o.inner.", 12), ["lo", "hi"]);
    assert_eq!(complete(&mut client, 6, "list(1).", 12), ["inner", "flag"]);
    assert_eq!(complete(&mut client, 8, "c.", 6), ["add", "get"]);
}

#[test]
fn signature_help_for_calls_and_maps() {
    let (_dir, mut client, user) = types_setup();
    let help = client.request("textDocument/signatureHelp", at(&user, 18, 18));
    let signatures = help["signatures"].as_array().unwrap();
    assert_eq!(signatures.len(), 1, "{help}");
    assert_eq!(
        signatures[0]["label"],
        "function scale(x : in integer; factor : in integer := 2) return integer"
    );
    assert_eq!(help["activeParameter"], 1);

    // A port map, named association.
    let help = client.request("textDocument/signatureHelp", at(&user, 22, 45));
    assert_eq!(
        help["signatures"][0]["label"],
        "entity leaf port map (a : in bit; y : out bit)"
    );
    assert_eq!(help["activeParameter"], 1, "{help}");

    // A call being typed.
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": user, "languageId": "vhdl", "version": 1, "text": TYPES_USE } }),
    );
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": user, "version": 2 },
            "contentChanges": [
                { "range": { "start": { "line": 20, "character": 4 }, "end": { "line": 20, "character": 9 } }, "text": "v := scale(v, " }
            ]
        }),
    );
    let help = client.request("textDocument/signatureHelp", at(&user, 20, 18));
    let signatures = help["signatures"].as_array().unwrap();
    assert_eq!(help["activeParameter"], 1, "{help}");
    assert!(
        signatures[help["activeSignature"].as_u64().unwrap() as usize]["label"]
            .as_str()
            .unwrap()
            .contains("factor"),
        "{help}"
    );

    // No argument yet: the overload the assignment target selects.
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": user, "version": 3 },
            "contentChanges": [
                { "range": { "start": { "line": 20, "character": 15 }, "end": { "line": 20, "character": 18 } }, "text": "" }
            ]
        }),
    );
    let help = client.request("textDocument/signatureHelp", at(&user, 20, 15));
    assert_eq!(help["signatures"].as_array().unwrap().len(), 1, "{help}");
    assert_eq!(help["activeParameter"], 0, "{help}");

    // Protected type methods.
    let help = client.request("textDocument/signatureHelp", at(&user, 19, 10));
    assert_eq!(
        help["signatures"][0]["label"],
        "procedure add(n : in integer)"
    );
}
