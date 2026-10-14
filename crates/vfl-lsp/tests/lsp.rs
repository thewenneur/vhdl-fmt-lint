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
        let r = self.response(method, params);
        assert!(r.error.is_none(), "{method}: {:?}", r.error);
        r.result.unwrap_or(Value::Null)
    }

    /// The error message of a request that fails.
    fn request_error(&mut self, method: &str, params: Value) -> String {
        let r = self.response(method, params);
        r.error.expect("the request succeeded").message
    }

    fn response(&mut self, method: &str, params: Value) -> lsp_server::Response {
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
                Message::Response(r) if r.id == id => return r,
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

/// Decodes semantic tokens into `(line, column, length, type, modifiers)`.
fn decode(data: &Value) -> Vec<[u64; 5]> {
    let ints: Vec<u64> = data
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    let (mut line, mut column) = (0, 0);
    ints.chunks(5)
        .map(|t| {
            line += t[0];
            column = if t[0] == 0 { column + t[1] } else { t[1] };
            [line, column, t[2], t[3], t[4]]
        })
        .collect()
}

#[test]
fn semantic_tokens_full_delta_and_range() {
    let (dir, _pkg, top) = setup();
    let mut client = Client::start(dir.path());
    let doc = json!({ "textDocument": { "uri": top } });

    let full = client.request("textDocument/semanticTokens/full", doc.clone());
    let tokens = decode(&full["data"]);
    // `width` in the port's range, a constant (type 12) of package `pkg`.
    let width = tokens
        .iter()
        .find(|t| t[0] == 2 && t[3] == 12)
        .expect("constant token");
    assert_eq!(width[1..3], [27, 5]);
    // `d`, an out port (type 11) being assigned: modification, out.
    let d = tokens.iter().find(|t| t[0] == 6).expect("port token");
    assert_eq!(d[1..4], [2, 1, 11]);
    assert_eq!(d[4], 1 << 3 | 1 << 5);

    // An edit that adds a signal: the delta is applied to the old tokens.
    let text = TOP.replace("is\nbegin", "is\n  signal s : bit;\nbegin");
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": top, "languageId": "vhdl", "version": 1, "text": text } }),
    );
    let delta = client.request(
        "textDocument/semanticTokens/full/delta",
        json!({ "textDocument": { "uri": top }, "previousResultId": full["resultId"] }),
    );
    let mut data: Vec<Value> = full["data"].as_array().unwrap().clone();
    for edit in delta["edits"].as_array().unwrap() {
        let start = edit["start"].as_u64().unwrap() as usize;
        let count = edit["deleteCount"].as_u64().unwrap() as usize;
        let new = edit["data"].as_array().cloned().unwrap_or_default();
        data.splice(start..start + count, new);
    }
    let whole = client.request("textDocument/semanticTokens/full", doc.clone());
    assert_eq!(Value::Array(data), whole["data"]);
    assert!(
        decode(&whole["data"])
            .iter()
            .any(|t| t[0] == 5 && t[3] == 10)
    );

    // A range gets only the tokens in it.
    let range = client.request(
        "textDocument/semanticTokens/range",
        json!({ "textDocument": { "uri": top }, "range": {
            "start": { "line": 5, "character": 0 }, "end": { "line": 6, "character": 0 } } }),
    );
    let lines: Vec<u64> = decode(&range["data"]).iter().map(|t| t[0]).collect();
    assert_eq!(lines, [5, 5]);
}

#[test]
fn code_generation_from_entities() {
    let (dir, _pkg, top) = setup();
    let mut client = Client::start(dir.path());

    let entities = client.request("vfl/entities", json!({}));
    assert_eq!(entities[0]["library"], "work");
    assert_eq!(entities[0]["name"], "top");

    let instance = client.request(
        "vfl/generate",
        json!({ "kind": "instance", "entity": "top" }),
    );
    assert_eq!(
        instance["text"],
        "u_top : entity work.top\n  port map (\n    d => d\n  );\n"
    );
    assert!(instance.get("path").is_none());
    let component = client.request(
        "vfl/generate",
        json!({ "kind": "component", "entity": "work.top" }),
    );
    assert!(
        component["text"]
            .as_str()
            .unwrap()
            .starts_with("component top is\n")
    );

    let testbench = client.request(
        "vfl/generate",
        json!({ "kind": "testbench", "entity": "top" }),
    );
    let tb_path = dir.path().join("top_tb.vhd");
    assert_eq!(testbench["path"], tb_path.to_str().unwrap());
    let text = testbench["text"].as_str().unwrap();
    assert!(text.contains("use work.pkg.all;"), "{text}");
    assert!(
        text.contains("signal d : bit_vector(width - 1 downto 0);"),
        "{text}"
    );

    // On the entity: a code action creates the testbench file.
    let actions = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": top },
            "range": { "start": { "line": 1, "character": 8 }, "end": { "line": 1, "character": 8 } },
            "context": { "diagnostics": [] }
        }),
    );
    let action = actions
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "refactor.generate.testbench")
        .unwrap_or_else(|| panic!("no testbench action in {actions}"));
    assert_eq!(
        action["title"],
        "Generate testbench for `top` in top_tb.vhd"
    );
    // The edit is computed only when the action is chosen.
    assert!(action.get("edit").is_none(), "{action}");
    let action = client.request("codeAction/resolve", action.clone());
    let changes = &action["edit"]["documentChanges"];
    assert_eq!(changes[0]["kind"], "create");
    assert_eq!(changes[0]["uri"], url(&tb_path));
    assert_eq!(changes[1]["edits"][0]["newText"], text);

    // Not elsewhere, and not once the file exists.
    let actions_at = |client: &mut Client, line: u32| {
        client
            .request(
                "textDocument/codeAction",
                json!({
                    "textDocument": { "uri": top },
                    "range": { "start": { "line": line, "character": 2 }, "end": { "line": line, "character": 2 } },
                    "context": { "diagnostics": [] }
                }),
            )
            .to_string()
    };
    assert!(!actions_at(&mut client, 6).contains("testbench"));
    std::fs::write(&tb_path, text).unwrap();
    assert!(!actions_at(&mut client, 1).contains("testbench"));

    let missing = client.request_error(
        "vfl/generate",
        json!({ "kind": "testbench", "entity": "nope" }),
    );
    assert!(missing.contains("no entity `nope`"), "{missing}");
}

const REFACTOR_PKG: &str = "\
package shapes is
  type point_t is record
    x : integer;
    y : integer;
  end record;
  function area(w : integer; h : integer) return integer;
  function area(side : integer) return integer;
end package shapes;
package body shapes is
  function area(w : integer; h : integer) return integer is
  begin
    return w * h;
  end function area;
  function area(side : integer) return integer is
  begin
    return side * side;
  end function area;
end package body shapes;
";

const REFACTOR_LEAF: &str = "\
entity leaf is
  generic (width : natural := 1);
  port (clk : in bit; q : out bit);
end entity leaf;
architecture rtl of leaf is
begin
  q <= clk;
end architecture rtl;
";

const REFACTOR_TOP: &str = "\
use work.shapes.all;
entity top is
  port (clk : in bit; o1, o2 : out bit);
end entity top;
architecture rtl of top is
  component leaf is
    generic (width : natural := 1);
    port (clk : in bit; q : out bit);
  end component;
  signal p : point_t := (x => 0, y => 0);
  signal n : integer;
begin
  u1 : entity work.leaf generic map (width => 2) port map (clk => clk, q => o1);
  u2 : leaf port map (clk => clk, q => o2);
  process (clk)
  begin
    p.x <= area(p.y, 2);
    n <= area(3);
  end process;
end architecture rtl;
";

/// The text of `original` with the edits of `edit` for `uri` applied.
fn apply(original: &str, edit: &Value, uri: &str) -> String {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(original.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let offset = |p: &Value| {
        starts[p["line"].as_u64().unwrap() as usize] + p["character"].as_u64().unwrap() as usize
    };
    let mut edits: Vec<(usize, usize, String)> = edit["changes"][uri]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|e| {
            (
                offset(&e["range"]["start"]),
                offset(&e["range"]["end"]),
                e["newText"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut text = original.to_owned();
    for (start, end, new) in edits {
        text.replace_range(start..end, &new);
    }
    text
}

/// The position of the `nth` occurrence of `needle` in `text`, plus `delta`.
fn find(uri: &str, text: &str, needle: &str, nth: usize, delta: usize) -> Value {
    let offset = text.match_indices(needle).nth(nth).unwrap().0 + delta;
    let line = text[..offset].matches('\n').count() as u32;
    let character = (offset - text[..offset].rfind('\n').map_or(0, |i| i + 1)) as u32;
    at(uri, line, character)
}

fn refactor_setup(config: &str) -> (tempfile::TempDir, Client, [String; 3]) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vfl.toml"), config).unwrap();
    std::fs::write(dir.path().join("shapes.vhd"), REFACTOR_PKG).unwrap();
    std::fs::write(dir.path().join("leaf.vhd"), REFACTOR_LEAF).unwrap();
    std::fs::write(dir.path().join("top.vhd"), REFACTOR_TOP).unwrap();
    let client = Client::start(dir.path());
    let uris = ["shapes.vhd", "leaf.vhd", "top.vhd"].map(|f| url(&dir.path().join(f)));
    (dir, client, uris)
}

fn rename(client: &mut Client, position: Value, new_name: &str) -> Value {
    let mut params = position;
    params["newName"] = json!(new_name);
    client.request("textDocument/rename", params)
}

#[test]
fn rename_covers_bodies_components_and_elements() {
    let (_dir, mut client, [pkg, leaf, top]) = refactor_setup("");

    // A subprogram: its declaration, its body, the body's end label and
    // the calls of that overload only.
    let edit = rename(
        &mut client,
        find(&pkg, REFACTOR_PKG, "area", 0, 0),
        "surface",
    );
    let renamed = apply(REFACTOR_PKG, &edit, &pkg);
    assert_eq!(renamed.matches("surface").count(), 3, "{renamed}");
    assert_eq!(
        renamed.matches("function area(side").count(),
        2,
        "{renamed}"
    );
    let renamed = apply(REFACTOR_TOP, &edit, &top);
    assert!(renamed.contains("p.x <= surface(p.y, 2);"), "{renamed}");
    assert!(renamed.contains("n <= area(3);"), "{renamed}");

    // The same from the body.
    let edit = rename(
        &mut client,
        find(&pkg, REFACTOR_PKG, "area", 4, 0),
        "square",
    );
    let renamed = apply(REFACTOR_PKG, &edit, &pkg);
    assert!(renamed.contains("function square(side : integer) return integer;"));
    assert!(renamed.contains("end function square;"), "{renamed}");
    assert!(apply(REFACTOR_TOP, &edit, &top).contains("n <= square(3);"));

    // A package: its body and both end labels.
    let edit = rename(
        &mut client,
        find(&pkg, REFACTOR_PKG, "shapes", 0, 0),
        "geometry",
    );
    let renamed = apply(REFACTOR_PKG, &edit, &pkg);
    assert_eq!(renamed.matches("geometry").count(), 4, "{renamed}");
    assert!(apply(REFACTOR_TOP, &edit, &top).starts_with("use work.geometry.all;"));

    // An entity's port: the component named after it and both port maps.
    let edit = rename(
        &mut client,
        find(&leaf, REFACTOR_LEAF, "clk", 0, 0),
        "clock",
    );
    assert!(apply(REFACTOR_LEAF, &edit, &leaf).contains("q <= clock;"));
    let renamed = apply(REFACTOR_TOP, &edit, &top);
    assert!(
        renamed.contains("port (clock : in bit; q : out bit);"),
        "{renamed}"
    );
    assert!(
        renamed.contains("port map (clock => clk, q => o1)"),
        "{renamed}"
    );
    assert!(
        renamed.contains("port map (clock => clk, q => o2)"),
        "{renamed}"
    );
    assert!(renamed.contains("port (clk : in bit; o1"), "{renamed}");

    // The entity itself: the component and its instantiation too.
    let edit = rename(&mut client, find(&top, REFACTOR_TOP, "leaf", 0, 0), "cell");
    assert!(apply(REFACTOR_LEAF, &edit, &leaf).contains("architecture rtl of cell is"));
    let renamed = apply(REFACTOR_TOP, &edit, &top);
    assert_eq!(renamed.matches("cell").count(), 3, "{renamed}");

    // A record element: its declaration, selected names and aggregates.
    let edit = rename(&mut client, find(&top, REFACTOR_TOP, "x =>", 0, 0), "col");
    assert!(apply(REFACTOR_PKG, &edit, &pkg).contains("    col : integer;"));
    let renamed = apply(REFACTOR_TOP, &edit, &top);
    assert!(renamed.contains("(col => 0, y => 0)"), "{renamed}");
    assert!(renamed.contains("p.col <= area(p.y, 2);"), "{renamed}");
    let mut references = find(&pkg, REFACTOR_PKG, "y :", 0, 0);
    references["context"] = json!({ "includeDeclaration": true });
    let references = client.request("textDocument/references", references);
    assert_eq!(references.as_array().unwrap().len(), 3, "{references}");
    let definition = client.request(
        "textDocument/definition",
        find(&top, REFACTOR_TOP, "p.y", 0, 2),
    );
    assert_eq!(definition[0]["uri"], pkg);

    // Names that would clash.
    let mut params = find(&top, REFACTOR_TOP, "n :", 0, 0);
    params["newName"] = json!("p");
    let error = client.request_error("textDocument/rename", params.clone());
    assert!(
        error.contains("signal `p` is already visible there"),
        "{error}"
    );
    params["newName"] = json!("o1");
    let error = client.request_error("textDocument/rename", params.clone());
    assert!(error.contains("port `o1`"), "{error}");
    params["newName"] = json!("process");
    let error = client.request_error("textDocument/rename", params.clone());
    assert!(error.contains("reserved word"), "{error}");
    let mut params = find(&pkg, REFACTOR_PKG, "x :", 0, 0);
    params["newName"] = json!("y");
    let error = client.request_error("textDocument/rename", params);
    assert!(
        error.contains("already has an element named `y`"),
        "{error}"
    );
}

const FIXES_TOP: &str = "\
library ieee;
use ieee.std_logic_1164.all;
use ieee.std_logic_arith.all;
entity top is
  port (clk : in std_logic; o : out std_logic);
end entity top;
architecture rtl of top is
\tsignal sDone : std_logic;
  signal unused_a, unused_b : std_logic;
  constant c_pi : real := math_pi;
begin
  u : entity work.leaf port map (clk, o);
end architecture rtl;
";

/// The quick fixes offered for the problems on `line`.
fn quick_fixes(client: &mut Client, uri: &str, line: u32) -> Vec<Value> {
    client
        .request(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": uri },
                "range": { "start": { "line": line, "character": 0 }, "end": { "line": line, "character": 80 } },
                "context": { "diagnostics": [], "only": ["quickfix"] }
            }),
        )
        .as_array()
        .unwrap()
        .clone()
}

fn fix<'a>(fixes: &'a [Value], title: &str) -> &'a Value {
    fixes
        .iter()
        .find(|f| f["title"] == title)
        .unwrap_or_else(|| panic!("no fix `{title}` in {fixes:#?}"))
}

#[test]
fn quick_fixes_for_rules() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("vfl.toml"),
        "[lint]\npreset = \"strict\"\n[lint.rules]\n\"naming/signal\" = { level = \"warn\", prefix = \"s_\", case = \"snake\" }\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("leaf.vhd"), REFACTOR_LEAF).unwrap();
    std::fs::write(dir.path().join("top.vhd"), FIXES_TOP).unwrap();
    let top = url(&dir.path().join("top.vhd"));
    let mut client = Client::start(dir.path());

    let fixes = quick_fixes(&mut client, &top, 2);
    let arith = fix(
        &fixes,
        "Use `ieee.numeric_std` instead (may change behavior)",
    );
    assert!(apply(FIXES_TOP, &arith["edit"], &top).contains("use ieee.numeric_std.all;"));
    let ignore = fix(&fixes, "Ignore `style/no-std-logic-arith` on this line");
    assert!(
        apply(FIXES_TOP, &ignore["edit"], &top)
            .contains("use ieee.std_logic_arith.all;  -- vfl-ignore style/no-std-logic-arith\n")
    );
    let ignore = fix(&fixes, "Ignore `style/no-std-logic-arith` in this file");
    assert!(
        apply(FIXES_TOP, &ignore["edit"], &top)
            .starts_with("-- vfl-ignore-file style/no-std-logic-arith\nlibrary ieee;")
    );

    // A tab, and a name against the convention: renamed everywhere.
    let fixes = quick_fixes(&mut client, &top, 7);
    let tabs = fix(&fixes, "Indent with spaces");
    assert!(apply(FIXES_TOP, &tabs["edit"], &top).contains("\n  signal sDone"));
    let rename = fix(&fixes, "Rename `sDone` to `s_s_done` everywhere");
    assert_eq!(rename["isPreferred"], true);
    // Worked out when chosen.
    assert!(rename.get("edit").is_none(), "{rename}");
    let rename = client.request("codeAction/resolve", rename.clone());
    assert!(apply(FIXES_TOP, &rename["edit"], &top).contains("signal s_s_done : std_logic;"));

    // Unused signals declared together.
    let fixes = quick_fixes(&mut client, &top, 8);
    let remove = fix(
        &fixes,
        "Remove the unused signal `unused_a` (may change behavior)",
    );
    assert!(apply(FIXES_TOP, &remove["edit"], &top).contains("  signal unused_b : std_logic;\n"));

    // `math_pi` is declared in `ieee.math_real`.
    let fixes = quick_fixes(&mut client, &top, 9);
    let import = fix(&fixes, "Add `use ieee.math_real.all;`");
    assert!(
        apply(FIXES_TOP, &import["edit"], &top)
            .contains("end entity top;\nuse ieee.math_real.all;\narchitecture rtl of top is")
    );

    // Positional associations get their formals.
    let fixes = quick_fixes(&mut client, &top, 11);
    let named = fix(&fixes, "Name the formals");
    assert!(apply(FIXES_TOP, &named["edit"], &top).contains("port map (clk => clk, q => o);"));
}

const VERILOG_FIFO: &str = "// A small FIFO.
module fifo #(parameter int DEPTH = 4) (
  input  logic       clk,
  input  logic [7:0] din,
  output logic [7:0] dout
);
endmodule
";

const VHDL_TOP: &str = "library ieee;
use ieee.std_logic_1164.all;
entity top is
  port (clk : in std_logic; q : out std_logic_vector(7 downto 0));
end entity;
architecture rtl of top is
begin
  u_fifo : entity work.fifo
    generic map (DEPTH => 8)
    port map (clk => clk, din => x\"00\", dout => q, full => open);
end architecture;
";

#[test]
fn verilog_modules_across_languages() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vfl.toml"), "").unwrap();
    std::fs::write(dir.path().join("fifo.sv"), VERILOG_FIFO).unwrap();
    std::fs::write(dir.path().join("top.vhd"), VHDL_TOP).unwrap();
    let fifo = url(&dir.path().join("fifo.sv"));
    let top = url(&dir.path().join("top.vhd"));
    let mut client = Client::start(dir.path());

    let messages = |client: &mut Client| -> Vec<String> {
        client
            .diagnostics(&top)
            .iter()
            .map(|d| d["message"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(messages(&mut client), ["module `fifo` has no port `full`"]);

    // The module, a parameter and a port, from the VHDL instance.
    let definition = client.request(
        "textDocument/definition",
        find(&top, VHDL_TOP, "work.fifo", 0, 6),
    );
    assert_eq!(definition[0]["uri"], fifo);
    assert_eq!(
        definition[0]["range"]["start"],
        json!({ "line": 1, "character": 7 })
    );
    let definition = client.request("textDocument/definition", find(&top, VHDL_TOP, "din", 0, 1));
    assert_eq!(
        definition[0]["range"]["start"],
        json!({ "line": 3, "character": 21 })
    );
    let position = find(&top, VHDL_TOP, "work.fifo", 0, 6);
    let text = hover_text(
        &mut client,
        &top,
        position["position"]["line"].as_u64().unwrap() as u32,
        position["position"]["character"].as_u64().unwrap() as u32,
    );
    assert!(
        text.starts_with("module `fifo` in library `work`"),
        "{text}"
    );
    assert!(text.contains("```systemverilog\nmodule fifo #("), "{text}");
    assert!(text.contains("output logic [7:0] dout\n);"), "{text}");
    let position = find(&top, VHDL_TOP, "din", 0, 1);
    let text = hover_text(
        &mut client,
        &top,
        position["position"]["line"].as_u64().unwrap() as u32,
        position["position"]["character"].as_u64().unwrap() as u32,
    );
    assert!(text.starts_with("port `din` in `work.fifo`"), "{text}");
    assert!(text.contains("input logic [7:0] din"), "{text}");
    let position = find(&top, VHDL_TOP, "DEPTH", 0, 1);
    let text = hover_text(
        &mut client,
        &top,
        position["position"]["line"].as_u64().unwrap() as u32,
        position["position"]["character"].as_u64().unwrap() as u32,
    );
    assert!(text.contains("parameter int DEPTH = 4"), "{text}");

    // Renaming would leave the Verilog side behind.
    let mut params = find(&top, VHDL_TOP, "din", 0, 1);
    params["newName"] = json!("data_in");
    let error = client.request_error("textDocument/rename", params);
    assert!(error.contains("Verilog"), "{error}");

    // The module gains the port: the error goes away.
    std::fs::write(
        dir.path().join("fifo.sv"),
        VERILOG_FIFO.replace("dout\n", "dout,\n  output logic full\n"),
    )
    .unwrap();
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": fifo, "type": 2 }] }),
    );
    assert_eq!(messages(&mut client), Vec::<String>::new());
}
