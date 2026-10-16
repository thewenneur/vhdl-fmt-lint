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
        Client::start_with(root, Value::Null)
    }

    /// Starts the server with the editor settings `options`.
    fn start_with(root: &Path, options: Value) -> Client {
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
            json!({
                "capabilities": {},
                "rootUri": root,
                "processId": null,
                "initializationOptions": options,
            }),
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

/// The format defaults before block alignment, which the expected texts
/// below are laid out with.
const LEGACY_FORMAT: &str =
    "[format]\nindent = 2\n[format.align]\nscope = \"contiguous\"\nsemicolon = false\n";

fn url(path: &Path) -> String {
    lsp_types::Url::from_file_path(path).unwrap().to_string()
}

const PKG: &str = "package pkg is\n  constant width : natural := 8;\nend package;\n";
const TOP: &str = "use work.pkg.all;\n\
entity top is\n  port (d : out bit_vector(width - 1 downto 0));\nend entity top;\n\
architecture rtl of top is\nbegin\n  d <= (others => '0');\nend architecture;\n";

fn setup() -> (tempfile::TempDir, String, String) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vfl.toml"), LEGACY_FORMAT).unwrap();
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
    std::fs::write(dir.path().join("vfl.toml"), LEGACY_FORMAT).unwrap();
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
    let (_dir, mut client, [pkg, leaf, top]) = refactor_setup(LEGACY_FORMAT);

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
        format!("{LEGACY_FORMAT}[lint]\npreset = \"strict\"\n[lint.rules]\n\"naming/signal\" = {{ level = \"warn\", prefix = \"s_\", case = \"snake\" }}\n"),
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
    // Names only, the default: VHDL resolves the module, nothing else is
    // done with it.
    std::fs::write(dir.path().join("vfl.toml"), LEGACY_FORMAT).unwrap();
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

/// Found by fuzzing: a word start after a multi-byte character.
#[test]
fn completion_after_a_non_ascii_character() {
    let mut world = vfl_lsp::World::detached();
    let path = std::env::temp_dir().join("vfl_completion_non_ascii.vhd");
    world.set_text(path.clone(), "entity e is end;\n--\u{93}x".into());
    let uri = lsp_types::Url::from_file_path(&path).unwrap().to_string();
    let completion = vfl_lsp::handle(&mut world, "textDocument/completion", at(&uri, 1, 4));
    assert!(completion.is_some());
    let completion = vfl_lsp::editor::completion(&mut world, &path, 21).unwrap();
    assert_eq!(completion.start, 21);
}

/// A world with no project, with `[verilog]` `mode = "full"` everywhere.
fn verilog_world() -> vfl_lsp::World {
    let mut config = vfl_config::Config::default();
    config.verilog.mode = vfl_config::VerilogMode::Full;
    vfl_lsp::World::detached_with(config, true)
}

/// Found by fuzzing: requests on a Verilog document, which has no VHDL
/// tree, panicked.
#[test]
fn requests_on_a_verilog_document() {
    let mut world = verilog_world();
    let path = std::env::temp_dir().join("vfl_requests_on_verilog.sv");
    world.set_text(path.clone(), "module m(input a);\nendmodule\n".into());
    world.refresh();
    let uri = lsp_types::Url::from_file_path(&path).unwrap().to_string();
    for method in [
        "textDocument/hover",
        "textDocument/completion",
        "textDocument/definition",
        "textDocument/signatureHelp",
    ] {
        vfl_lsp::handle(&mut world, method, at(&uri, 0, 8));
    }
    assert!(vfl_lsp::editor::hover(&mut world, &path, 8).is_none());
}

/// Verilog documents get lint problems, their fixes and comments that
/// silence them in `//` comments.
#[test]
fn verilog_problems_and_fixes() {
    let mut world = verilog_world();
    let path = std::env::temp_dir().join("vfl_verilog_problems.sv");
    let text = "module m (input wire clk, input wire d, output reg q);\n  \
                always @(posedge clk) q = d;\nendmodule\n";
    world.set_text(path.clone(), text.into());
    let problems = vfl_lsp::editor::problems(&mut world);
    let rules: Vec<&str> = problems.iter().map(|p| p.rule).collect();
    assert_eq!(rules, ["verilog/blocking-in-clocked"], "{problems:?}");
    let at = text.find(" = d").unwrap() + 1;
    let fixes = vfl_lsp::editor::quick_fixes(&mut world, &path, at, at);
    let titles: Vec<&str> = fixes.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Use a non-blocking assignment (may change behavior)",
            "Ignore `verilog/blocking-in-clocked` on this line",
            "Ignore `verilog/blocking-in-clocked` in this file",
        ]
    );
    assert_eq!(fixes[0].edits, [(at..at + 1, "<=".to_owned())]);
    assert_eq!(
        fixes[1].edits[0].1,
        "  // vfl-ignore verilog/blocking-in-clocked"
    );
}

const ASSOCIATE_SUB: &str = "\
entity sub is
  generic (N : natural; M : natural := 2);
  port (a : in bit; b : in bit; c : in bit := '0'; y : out bit; z : out bit);
end entity;
architecture rtl of sub is
begin
end architecture;
";

const ASSOCIATE_TOP: &str = "\
entity top is
end entity;
architecture rtl of top is
  signal a, b, y, z : bit;
begin
  u1 : entity work.sub generic map (N => 3) port map (a => a, y => y);
  u2 : entity work.sub
    generic map (
      N => 1
    )
    port map (
      a     => a,
      y     => y,
      inst  => open
    );
  u3 : entity work.sub;
  u4 : entity work.sub generic map (5) port map (a, open, open, y);
end architecture;
";

#[test]
fn quick_fixes_associate_left_out_ports() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vfl.toml"), LEGACY_FORMAT).unwrap();
    std::fs::write(dir.path().join("sub.vhd"), ASSOCIATE_SUB).unwrap();
    std::fs::write(dir.path().join("top.vhd"), ASSOCIATE_TOP).unwrap();
    let top = url(&dir.path().join("top.vhd"));
    let mut client = Client::start(dir.path());
    let resolve = |client: &mut Client, fixes: &[Value], title: &str| {
        let action = fix(fixes, title);
        // Worked out when chosen.
        assert!(action.get("edit").is_none(), "{action}");
        let action = client.request("codeAction/resolve", action.clone());
        apply(ASSOCIATE_TOP, &action["edit"], &top)
    };

    // On one line: added on the line.
    let fixes = quick_fixes(&mut client, &top, 5);
    assert_eq!(fix(&fixes, "Associate port `b`")["isPreferred"], true);
    let fixed = resolve(&mut client, &fixes, "Associate port `b`");
    assert!(
        fixed.contains("port map (a => a, y => y, b => b);"),
        "{fixed}"
    );
    let fixed = resolve(&mut client, &fixes, "Associate every port");
    // Any port without an object of its name is left open.
    assert!(
        fixed.contains("port map (a => a, y => y, b => b, c => open, z => z);"),
        "{fixed}"
    );

    // One per line, aligned: the arrows stay aligned.
    let fixes = quick_fixes(&mut client, &top, 10);
    let fixed = resolve(&mut client, &fixes, "Associate port `b`");
    assert!(
        fixed.contains("      inst  => open,\n      b     => b\n    );"),
        "{fixed}"
    );

    // No map at all: one for each.
    let fixes = quick_fixes(&mut client, &top, 15);
    let fixed = resolve(&mut client, &fixes, "Associate generic `N`");
    assert!(
        fixed.contains("  u3 : entity work.sub\n    generic map (\n      N => open\n    );"),
        "{fixed}"
    );
    let fixed = resolve(&mut client, &fixes, "Associate ports `a`, `b`");
    assert!(
        fixed.contains(
            "  u3 : entity work.sub\n    port map (\n      a => a,\n      b => b\n    );"
        ),
        "{fixed}"
    );

    // Positional associations count.
    let fixes = quick_fixes(&mut client, &top, 16);
    assert!(
        fixes
            .iter()
            .all(|f| !f["title"].as_str().unwrap().starts_with("Associate")),
        "{fixes:#?}"
    );
}

const ASSOCIATE_FORMATTED: &str = "\
entity top is
end entity;
architecture rtl of top is
  signal a, y : bit;
begin
  u2 : entity work.sub
    generic map (
      N => 1
    )
    port map (
      a => a
    , y => y
    );
  u3 : entity work.sub;
end architecture;
";

/// In a formatted file the fix keeps it formatted (here with leading
/// commas), in the editors that link vfl too.
#[test]
fn associating_ports_keeps_the_format() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("vfl.toml"),
        "[format]\nassociation_separator = \"leading\"\nindent = 2\n\
         [format.align]\nscope = \"contiguous\"\nsemicolon = false\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("sub.vhd"),
        ASSOCIATE_SUB.replace("z : out bit", "long_name : out bit"),
    )
    .unwrap();
    let path = dir.path().join("top.vhd");
    std::fs::write(&path, ASSOCIATE_FORMATTED).unwrap();
    let mut world = vfl_lsp::World::new(dir.path());
    let fixed = |world: &mut vfl_lsp::World, at: usize, title: &str| {
        let fixes = vfl_lsp::editor::quick_fixes(world, &path, at, at);
        let fix = fixes
            .iter()
            .find(|f| f.title == title)
            .unwrap_or_else(|| panic!("no fix `{title}`"));
        let mut text = ASSOCIATE_FORMATTED.to_owned();
        for (range, new) in fix.edits.iter().rev() {
            text.replace_range(range.clone(), new);
        }
        text
    };

    let at = ASSOCIATE_FORMATTED.find("a => a").unwrap();
    let text = fixed(&mut world, at, "Associate every port");
    assert!(
        text.contains(
            "    port map (
      a         => a
    , y         => y
    , b         => open
    , c         => open
    , long_name => open
    );"
        ),
        "{text}"
    );

    let at = ASSOCIATE_FORMATTED.find("u3").unwrap();
    let text = fixed(&mut world, at, "Associate ports `a`, `b`");
    assert!(
        text.contains(
            "  u3 : entity work.sub\n    port map (\n      a => a\n    , b => open\n    );"
        ),
        "{text}"
    );

    // Configured to name the required ones, to be filled in.
    std::fs::write(
        dir.path().join("vfl.toml"),
        "[format]\nassociation_separator = \"leading\"\nindent = 2\n\
         [format.align]\nscope = \"contiguous\"\nsemicolon = false\n\
         [lint.rules]\n\"sema/unassociated-port\" = { undeclared = \"name\" }\n",
    )
    .unwrap();
    let mut world = vfl_lsp::World::new(dir.path());
    let text = fixed(&mut world, at, "Associate ports `a`, `b`");
    assert!(
        text.contains("  u3 : entity work.sub\n    port map (\n      a => a\n    , b => b\n    );"),
        "{text}"
    );
}

/// The editor's settings are the config of a project without a config
/// file, and become its `vfl.toml` on request.
#[test]
fn editor_settings_apply_without_a_config_file() {
    let dir = tempfile::tempdir().unwrap();
    if !vfl_config::config_chain(dir.path()).unwrap().is_empty() {
        return; // A stray config above the temporary directory.
    }
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let path = dir.path().join("a.vhd");
    let text = "entity a is\nend entity;\narchitecture rtl of a is\n  signal s : bit;\nbegin\nend architecture;\n";
    std::fs::write(&path, text).unwrap();
    let uri = url(&path);
    let mut client = Client::start_with(
        dir.path(),
        json!({ "config": { "lint": { "preset": "none" }, "format": { "style": "classic" } } }),
    );
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "vhdl", "version": 0, "text": text } }),
    );
    assert_eq!(client.diagnostics(&uri), Vec::<Value>::new());
    let edits = client.request(
        "textDocument/formatting",
        json!({ "textDocument": { "uri": uri }, "options": { "tabSize": 4, "insertSpaces": true } }),
    );
    assert!(edits.to_string().contains("ENTITY"), "{edits}");

    // A settings change applies right away.
    client.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": { "config": { "lint": { "preset": "strict" } } } }),
    );
    let rules: Vec<Value> = client
        .diagnostics(&uri)
        .iter()
        .map(|d| d["code"].clone())
        .collect();
    assert!(rules.contains(&json!("sema/unused-signal")), "{rules:?}");

    // The settings in effect, as a config file.
    let generated = client.request("vfl/projectConfig", Value::Null);
    assert_eq!(generated["exists"], json!(false));
    let target = dir.path().join("vfl.toml");
    assert_eq!(generated["path"], json!(target));
    let text = generated["text"].as_str().unwrap();
    assert!(text.contains("preset = \"strict\""), "{text}");
    let actions = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri },
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
            "context": { "diagnostics": [], "only": ["source.vfl.generateConfig"] },
        }),
    );
    let action = actions[0].clone();
    assert_eq!(action["kind"], json!("source.vfl.generateConfig"));
    let resolved = client.request("codeAction/resolve", action);
    let changes = &resolved["edit"]["documentChanges"];
    assert_eq!(changes[0]["kind"], json!("create"));
    assert_eq!(changes[1]["edits"][0]["newText"], json!(text));

    // Once written, the file wins over the editor's settings.
    std::fs::write(&target, "[lint]\npreset = \"none\"\n").unwrap();
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": url(&target), "type": 1 }] }),
    );
    assert_eq!(client.diagnostics(&uri), Vec::<Value>::new());
    let generated = client.request("vfl/projectConfig", Value::Null);
    assert_eq!(generated["exists"], json!(true));
    let actions = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri },
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
            "context": { "diagnostics": [], "only": ["source.vfl.generateConfig"] },
        }),
    );
    assert_eq!(actions, json!([]));
}

/// Multi-byte text before names: no-break spaces (two bytes, one UTF-16
/// unit), an ideographic space (three bytes), accents and an emoji (four
/// bytes, a UTF-16 surrogate pair).
const UNICODE: &str = concat!(
    "entity u is\nend entity;\narchitecture rtl of u is\n",
    "  -- caf\u{e9} \u{4e2d}\u{6587} \u{1f600}\n",
    "  function f(a : integer;\n",
    " \u{a0}\u{a0}b : integer)\n",
    " \u{a0}return integer is\n",
    "  begin\n    return a + b;\n  end function;\n",
    "  constant s : string := \"\u{e9}t\u{e9}\";\n",
    "  constant s2 : string := \"\u{e9}\u{1f600}\" & s;\n",
    "begin\n",
    "\u{3000}assert f(1, 2) = 3; -- \u{1f600}\n",
    "end architecture;\n",
);

/// The LSP position (UTF-16) of the `nth` occurrence of `needle`, plus
/// `delta` bytes.
fn find_utf16(uri: &str, text: &str, needle: &str, nth: usize, delta: usize) -> Value {
    let offset = text.match_indices(needle).nth(nth).unwrap().0 + delta;
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line = text[..offset].matches('\n').count() as u32;
    at(
        uri,
        line,
        text[line_start..offset].encode_utf16().count() as u32,
    )
}

/// Applies the edits of a workspace edit, with UTF-16 positions.
fn apply_utf16(original: &str, edit: &Value, uri: &str) -> String {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(original.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let offset = |p: &Value| {
        let mut at = starts[p["line"].as_u64().unwrap() as usize];
        let mut units = p["character"].as_u64().unwrap() as usize;
        for c in original[at..].chars() {
            if units == 0 {
                break;
            }
            units -= c.len_utf16();
            at += c.len_utf8();
        }
        at
    };
    let mut edits: Vec<(usize, usize, String)> = edit["changes"][uri]
        .as_array()
        .unwrap()
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

#[test]
fn positions_on_lines_with_multi_byte_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("u.vhd");
    std::fs::write(&path, UNICODE).unwrap();
    let uri = url(&path);
    let mut client = Client::start(dir.path());

    // A function whose declaration continues on lines indented with
    // no-break spaces: the hover re-indents it (it crashed the server).
    let call = find_utf16(&uri, UNICODE, "f(1", 0, 0);
    assert_eq!(call["position"], json!({ "line": 13, "character": 8 }));
    let hover = client.request("textDocument/hover", call.clone());
    let value = hover["contents"]["value"].as_str().unwrap();
    assert!(
        value.contains("function f(a : integer;\n\u{a0}b : integer)\nreturn integer is\n"),
        "{value}"
    );
    assert_eq!(hover["range"]["start"], call["position"]);

    // `s` after an emoji: one more UTF-16 unit than characters.
    let s = find_utf16(&uri, UNICODE, "& s;", 0, 2);
    let hover = client.request("textDocument/hover", s.clone());
    assert_eq!(hover["range"]["start"], s["position"]);
    let value = hover["contents"]["value"].as_str().unwrap();
    assert!(value.contains("constant s : string"), "{value}");

    // Semantic tokens start at UTF-16 columns.
    let full = client.request(
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": uri } }),
    );
    let tokens = decode(&full["data"]);
    for position in [&s["position"], &call["position"]] {
        let (line, character) = (position["line"].as_u64(), position["character"].as_u64());
        assert!(
            tokens
                .iter()
                .any(|t| Some(t[0]) == line && Some(t[1]) == character && t[2] == 1),
            "{position} in {tokens:?}"
        );
    }

    // Renaming `s` edits its declaration and the use after the emoji.
    let edit = rename(&mut client, s, "text");
    assert_eq!(
        apply_utf16(UNICODE, &edit, &uri),
        UNICODE
            .replace("constant s :", "constant text :")
            .replace("& s;", "& text;")
    );
}

const CASE_TOP: &str = "library ieee;
use ieee.std_logic_1164.all;
entity cases is
  port (s : in std_logic; q : out std_logic);
end entity;
architecture rtl of cases is
  type state_t is (idle, run, done);
  signal state : state_t;
begin
  process (all)
  begin
    case state is
      when idle => q <= '0';
    end case;
    case s is
      when '0' => q <= '1';
    end case;
  end process;
end architecture;
";

#[test]
fn quick_fixes_add_missing_case_alternatives() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vfl.toml"), "").unwrap();
    std::fs::write(dir.path().join("cases.vhd"), CASE_TOP).unwrap();
    let uri = url(&dir.path().join("cases.vhd"));
    let mut client = Client::start(dir.path());

    let fixes = quick_fixes(&mut client, &uri, 11);
    let add = fix(
        &fixes,
        "Add `when` alternatives for the 2 missing choices (may change behavior)",
    );
    assert!(
        apply(CASE_TOP, &add["edit"], &uri).contains(
            "      when idle => q <= '0';\n      when run => null;\n      when done => null;\n    end case;"
        )
    );
    // `std_logic` has 9 values: `when others`.
    let fixes = quick_fixes(&mut client, &uri, 14);
    let others = fix(&fixes, "Add `when others => null;` (may change behavior)");
    assert!(
        apply(CASE_TOP, &others["edit"], &uri)
            .contains("      when '0' => q <= '1';\n      when others => null;\n    end case;")
    );
}

/// A hierarchy as indented lines: `label: name(architecture)`, with
/// the kind of grouping nodes and the flags of the others.
fn outline(nodes: &Value) -> String {
    fn walk(nodes: &Value, depth: usize, out: &mut String) {
        for node in nodes.as_array().unwrap() {
            let mut line = format!("{}{}", "  ".repeat(depth), node["label"].as_str().unwrap());
            match node["kind"].as_str().unwrap() {
                "generate" | "block" => {
                    line += &format!(" [{}", node["kind"].as_str().unwrap());
                    if let Some(detail) = node["detail"].as_str() {
                        line += &format!(" {detail}");
                    }
                    line += "]";
                }
                kind => {
                    if kind == "instance" {
                        line += &format!(": {}", node["name"].as_str().unwrap());
                    }
                    if let Some(arch) = node["architecture"].as_str() {
                        line += &format!("({arch})");
                    }
                    for flag in ["unresolved", "recursive", "truncated", "testbench"] {
                        if node[flag] == true {
                            line += &format!(" {flag}");
                        }
                    }
                }
            }
            out.push_str(&line);
            out.push('\n');
            walk(&node["children"], depth + 1, out);
        }
    }
    let mut out = String::new();
    walk(nodes, 0, &mut out);
    out
}

#[test]
fn design_hierarchy_of_the_uart_example() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/uart");
    let root = std::fs::canonicalize(root).unwrap();
    let mut client = Client::start(&root);

    let tree = client.request("vfl/hierarchy", json!({}));
    assert_eq!(
        outline(&tree["roots"]),
        "tb_uart(sim) testbench\n\
         \x20 dut: uart(rtl)\n\
         \x20   tx_fifo: fifo(rtl)\n\
         \x20   transmitter: uart_tx(rtl)\n\
         \x20   receiver: uart_rx(rtl)\n\
         \x20   error_sync: pulse_sync(rtl)\n\
         \x20   rx_fifo: fifo(rtl)\n"
    );
    let entities: Vec<(&str, bool)> = tree["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["name"].as_str().unwrap(), e["top"] == true))
        .collect();
    assert_eq!(
        entities,
        [
            ("fifo", false),
            ("pulse_sync", false),
            ("tb_uart", true),
            ("uart", false),
            ("uart_rx", false),
            ("uart_tx", false),
        ]
    );

    // Each instance points at its label, and at the entity it binds to.
    let dut = &tree["roots"][0]["children"][0];
    assert_eq!(dut["unitKind"], "entity");
    assert_eq!(dut["library"], "work");
    assert_eq!(dut["location"]["uri"], url(&root.join("tb/tb_uart.vhd")));
    assert_eq!(
        dut["location"]["range"]["start"],
        json!({ "line": 21, "character": 4 })
    );
    assert_eq!(
        dut["entityLocation"]["uri"],
        url(&root.join("rtl/uart.vhd"))
    );
    assert_eq!(dut["entityLocation"]["range"]["start"]["line"], 9);
    assert_eq!(dut["architectureLocation"]["range"]["start"]["line"], 34);

    // Without testbenches, what they instantiate is the top.
    let tree = client.request("vfl/hierarchy", json!({ "testbenches": false }));
    assert_eq!(tree["roots"].as_array().unwrap().len(), 1);
    assert_eq!(tree["roots"][0]["label"], "uart");
    assert_eq!(tree["roots"][0]["children"].as_array().unwrap().len(), 5);

    // A chosen top, by name or library and name.
    let tree = client.request("vfl/hierarchy", json!({ "top": "work.UART_TX" }));
    assert_eq!(outline(&tree["roots"]), "uart_tx(rtl)\n");
    let tree = client.request("vfl/hierarchy", json!({ "top": "nothing" }));
    assert_eq!(tree["roots"], json!([]));
    assert_eq!(tree["message"], "no entity `nothing` in the project");
}

const HIER_LEAF: &str = "entity leaf is\n  port (a : in bit);\nend entity;\n\
architecture rtl of leaf is\nbegin\nend architecture;\n\
architecture fast of leaf is\nbegin\nend architecture;\n\
configuration leaf_cfg of leaf is\n  for rtl\n  end for;\nend configuration;\n";

const HIER_TOP: &str = "entity top is\n  port (a : in bit);\nend entity;\n\
architecture rtl of top is\n\
  component leaf is port (a : in bit); end component;\n\
  component black_box is port (a : in bit); end component;\n\
begin\n\
  gen : for i in 0 to 3 generate\n\
    u_leaf : leaf port map (a => a);\n\
  end generate;\n\
  g2 : if true generate\n\
    u1 : entity work.leaf(rtl) port map (a => a);\n\
  else generate\n\
    u2 : configuration work.leaf_cfg port map (a => a);\n\
  end generate;\n\
  b : block begin\n\
    u3 : entity work.missing;\n\
    u4 : black_box port map (a => a);\n\
  end block;\n\
  self : entity work.top port map (a => a);\n\
end architecture;\n";

#[test]
fn design_hierarchy_groups_generates_and_flags_unresolved_instances() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vfl.toml"), "").unwrap();
    std::fs::write(dir.path().join("leaf.vhd"), HIER_LEAF).unwrap();
    std::fs::write(dir.path().join("top.vhd"), HIER_TOP).unwrap();
    let mut client = Client::start(dir.path());

    // `top` instantiates itself: it is still a top, not expanded twice.
    let tree = client.request("vfl/hierarchy", Value::Null);
    assert_eq!(
        outline(&tree["roots"]),
        "top(rtl)\n\
         \x20 gen [generate for i in 0 to 3]\n\
         \x20   u_leaf: leaf(fast)\n\
         \x20 g2 [generate if true]\n\
         \x20   u1: leaf(rtl)\n\
         \x20   u2: leaf(rtl)\n\
         \x20 b [block]\n\
         \x20   u3: missing unresolved\n\
         \x20   u4: black_box unresolved\n\
         \x20 self: top(rtl) recursive\n"
    );
    let g2 = &tree["roots"][0]["children"][1];
    assert_eq!(g2["children"][0]["unitKind"], "entity");
    assert_eq!(g2["children"][1]["unitKind"], "configuration");
    assert_eq!(
        tree["roots"][0]["children"][0]["children"][0]["unitKind"],
        "component"
    );
}

const NAV_PKG: &str = "\
package util is
  type pair is record
    lo : bit;
  end record pair;
  function f(a : integer; b : bit) return integer;
  procedure p(x : integer);
end package util;
package body util is
  function f(a : integer; b : bit) return integer is
  begin
    return a;
  end function f;
  procedure p(x : integer) is
    variable v : integer;
  begin
    v := f(x, '1');
  end procedure p;
end package body util;
";

const NAV_SUB: &str = "\
entity sub is
  generic (w : natural := 1);
  port (i : in bit; o : out bit);
end entity sub;
architecture rtl of sub is
begin
  o <= i;
end architecture rtl;
";

const NAV_TOP: &str = "\
use work.util.all;
entity top is
end entity top;
architecture rtl of top is
  component sub is
    generic (w : natural := 1);
    port (i : in bit; o : out bit);
  end component;
  signal i, t : bit;
  signal n : integer;
  signal r : pair;
begin
  u1: entity work.sub generic map (2) port map (i, t);
  u2: sub port map (i => i, o => t);
  main: process
  begin
    n <= f(1, '0');
    p(n);
    wait;
  end process main;
end architecture rtl;
";

fn nav_setup(config: &str, options: Value) -> (tempfile::TempDir, Client, [String; 3]) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vfl.toml"), config).unwrap();
    std::fs::write(dir.path().join("util.vhd"), NAV_PKG).unwrap();
    std::fs::write(dir.path().join("sub.vhd"), NAV_SUB).unwrap();
    std::fs::write(dir.path().join("top.vhd"), NAV_TOP).unwrap();
    let client = Client::start_with(dir.path(), options);
    let uris = ["util.vhd", "sub.vhd", "top.vhd"].map(|f| url(&dir.path().join(f)));
    (dir, client, uris)
}

fn hints(client: &mut Client, uri: &str, lines: std::ops::Range<u32>) -> Vec<(u32, u32, String)> {
    let result = client.request(
        "textDocument/inlayHint",
        json!({
            "textDocument": { "uri": uri },
            "range": {
                "start": { "line": lines.start, "character": 0 },
                "end": { "line": lines.end, "character": 0 },
            },
        }),
    );
    result
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            (
                h["position"]["line"].as_u64().unwrap() as u32,
                h["position"]["character"].as_u64().unwrap() as u32,
                h["label"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn hint_labels(client: &mut Client, uri: &str, lines: std::ops::Range<u32>) -> Vec<String> {
    hints(client, uri, lines)
        .into_iter()
        .map(|(_, _, l)| l)
        .collect()
}

#[test]
fn inlay_hints_name_positional_formals() {
    let (_dir, mut client, [util, _, top]) = nav_setup("", Value::Null);
    let all = hints(&mut client, &top, 0..30);
    let labels: Vec<&str> = all.iter().map(|(_, _, l)| l.as_str()).collect();
    // `i` is already the port's name; named associations need no hint.
    assert_eq!(labels, ["w:", "o:", "a:", "b:", "x:"], "{all:?}");
    assert_eq!(all[0], (12, 35, "w:".into()));
    // Only the lines asked for.
    let line = hints(&mut client, &top, 16..17);
    assert_eq!(line, [(16, 11, "a:".into()), (16, 14, "b:".into())]);
    let body = hints(&mut client, &util, 15..16);
    assert_eq!(body, [(15, 11, "a:".into()), (15, 14, "b:".into())]);

    // The project turns some off and port types on.
    let (_dir, mut client, [_, _, top]) = nav_setup(
        "[lsp.inlay_hints]\nparameters = false\nport_types = true\n",
        Value::Null,
    );
    assert_eq!(
        hint_labels(&mut client, &top, 12..14),
        ["w:", ": in bit", "o:", ": out bit", ": in bit", ": out bit"]
    );
    assert!(hints(&mut client, &top, 16..18).is_empty());

    // The editor's settings win over the project's.
    let (_dir, mut client, [_, _, top]) = nav_setup(
        "[lsp.inlay_hints]\nparameters = false\n",
        json!({ "inlayHints": { "parameters": true, "ports": false, "generics": false } }),
    );
    assert_eq!(hint_labels(&mut client, &top, 0..30), ["a:", "b:", "x:"]);
    client.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": { "inlayHints": { "parameters": false } } }),
    );
    assert_eq!(hint_labels(&mut client, &top, 0..30), ["w:", "o:"]);
}

/// File name, line and character of each location.
fn targets(result: &Value) -> Vec<(String, u64, u64)> {
    result
        .as_array()
        .map(|a| {
            a.iter()
                .map(|l| {
                    let uri = l["uri"].as_str().unwrap();
                    (
                        uri.rsplit('/').next().unwrap().to_owned(),
                        l["range"]["start"]["line"].as_u64().unwrap(),
                        l["range"]["start"]["character"].as_u64().unwrap(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn implementation_declaration_and_type_definition() {
    let (_dir, mut client, [util, sub, top]) = nav_setup("", Value::Null);
    let to = |file: &str, line, character| vec![(file.to_owned(), line, character)];
    let mut go = |method: &str, position: Value| targets(&client.request(method, position));
    // Entity to architecture, from its declaration and from an instance.
    let implementation = "textDocument/implementation";
    assert_eq!(go(implementation, at(&sub, 0, 8)), to("sub.vhd", 4, 13));
    let instance = find(&top, NAV_TOP, "work.sub", 0, 6);
    assert_eq!(go(implementation, instance), to("sub.vhd", 4, 13));
    // Component to entity.
    assert_eq!(go(implementation, at(&top, 13, 7)), to("sub.vhd", 0, 7));
    // Package to body, subprogram to body.
    assert_eq!(go(implementation, at(&top, 0, 10)), to("util.vhd", 7, 13));
    assert_eq!(go(implementation, at(&top, 16, 9)), to("util.vhd", 8, 11));

    // Body to declaration; architecture to entity.
    let declaration = "textDocument/declaration";
    assert_eq!(go(declaration, at(&util, 12, 12)), to("util.vhd", 5, 12));
    assert_eq!(go(declaration, at(&sub, 4, 14)), to("sub.vhd", 0, 7));

    // An object to its type; a record element to its type.
    let type_definition = "textDocument/typeDefinition";
    assert_eq!(go(type_definition, at(&top, 10, 9)), to("util.vhd", 1, 7));
    let element = go(type_definition, at(&util, 2, 4));
    assert!(element[0].0.ends_with("standard.vhd"), "{element:?}");
}

fn linked(client: &mut Client, uri: &str, line: u32, character: u32) -> Vec<(u64, u64)> {
    let r = client.request("textDocument/linkedEditingRange", at(uri, line, character));
    r["ranges"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| {
                    let start = &r["start"];
                    (
                        start["line"].as_u64().unwrap(),
                        start["character"].as_u64().unwrap(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn linked_editing_and_selection_ranges() {
    let (_dir, mut client, [util, sub, top]) = nav_setup("", Value::Null);
    // A label, from either end.
    assert_eq!(linked(&mut client, &top, 14, 3), [(14, 2), (19, 14)]);
    assert_eq!(linked(&mut client, &top, 19, 15), [(14, 2), (19, 14)]);
    // Units, subprogram bodies, record types.
    assert_eq!(linked(&mut client, &sub, 0, 8), [(0, 7), (3, 11)]);
    assert_eq!(linked(&mut client, &sub, 4, 13), [(4, 13), (7, 17)]);
    assert_eq!(linked(&mut client, &util, 12, 13), [(12, 12), (16, 16)]);
    assert_eq!(linked(&mut client, &util, 1, 8), [(1, 7), (3, 13)]);
    // Not a name with an `end` name.
    assert!(linked(&mut client, &top, 8, 9).is_empty());
    assert!(linked(&mut client, &sub, 4, 21).is_empty());

    let result = client.request(
        "textDocument/selectionRange",
        json!({
            "textDocument": { "uri": top },
            "positions": [{ "line": 16, "character": 11 }],
        }),
    );
    let mut chain = Vec::new();
    let mut node = &result[0];
    while !node.is_null() {
        let r = &node["range"];
        chain.push((
            r["start"]["line"].as_u64().unwrap(),
            r["start"]["character"].as_u64().unwrap(),
            r["end"]["line"].as_u64().unwrap(),
            r["end"]["character"].as_u64().unwrap(),
        ));
        node = &node["parent"];
    }
    // `1`, then the call, the assignment, the process, the architecture.
    assert_eq!(chain[0], (16, 11, 16, 12));
    for wanted in [
        (16, 9, 16, 18),
        (16, 4, 16, 19),
        (14, 2, 19, 19),
        (3, 0, 20, 21),
    ] {
        assert!(chain.contains(&wanted), "{wanted:?} not in {chain:?}");
    }
    for pair in chain.windows(2) {
        assert!(pair[0] != pair[1], "{chain:?}");
    }
}

#[test]
fn code_lenses_count_instances() {
    let (_dir, mut client, [_, sub, top]) = nav_setup("", Value::Null);
    let document = |uri: &str| json!({ "textDocument": { "uri": uri } });
    let lenses = client.request("textDocument/codeLens", document(&sub));
    let lenses = lenses.as_array().unwrap();
    assert_eq!(lenses.len(), 1);
    assert!(lenses[0].get("command").is_none());
    let resolved = client.request("codeLens/resolve", lenses[0].clone());
    // One entity instantiation, one through the component named after it.
    assert_eq!(resolved["command"]["title"], "2 instances");
    assert_eq!(resolved["command"]["command"], vfl_lsp::SHOW_REFERENCES);
    let arguments = &resolved["command"]["arguments"];
    assert_eq!(arguments[0], sub);
    assert_eq!(arguments[1], json!({ "line": 0, "character": 7 }));
    assert_eq!(
        targets(&arguments[2]),
        [
            ("top.vhd".to_owned(), 12, 18),
            ("top.vhd".to_owned(), 13, 6)
        ]
    );

    let lenses = client.request("textDocument/codeLens", document(&top));
    let mut titles = Vec::new();
    for lens in lenses.as_array().unwrap() {
        let resolved = client.request("codeLens/resolve", lens.clone());
        titles.push(resolved["command"]["title"].as_str().unwrap().to_owned());
    }
    // `top`, then the component.
    assert_eq!(titles, ["0 instances", "1 instance"]);
}

/// The name of the `key` item of each call, and how many ranges it has.
fn calls(calls: &Value, key: &str) -> Vec<(String, usize)> {
    calls
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c[key]["name"].as_str().unwrap().to_owned(),
                c["fromRanges"].as_array().unwrap().len(),
            )
        })
        .collect()
}

#[test]
fn call_hierarchy_of_subprograms_and_instances() {
    let (_dir, mut client, [util, sub, top]) = nav_setup("", Value::Null);

    // `f`, from a call: the item is its body.
    let items = client.request("textDocument/prepareCallHierarchy", at(&top, 16, 9));
    assert_eq!(items[0]["name"], "f");
    assert_eq!(items[0]["uri"], util);
    let selection = &items[0]["selectionRange"]["start"];
    assert_eq!(*selection, json!({ "line": 8, "character": 11 }));
    let incoming = client.request("callHierarchy/incomingCalls", json!({ "item": items[0] }));
    // By file: top.vhd, then util.vhd.
    assert_eq!(
        calls(&incoming, "from"),
        [("main".into(), 1), ("p".into(), 1)]
    );
    // What `p` calls, and what the process calls.
    let p = json!({ "item": incoming[1]["from"] });
    let outgoing = client.request("callHierarchy/outgoingCalls", p);
    assert_eq!(calls(&outgoing, "to"), [("f".into(), 1)]);
    let main = json!({ "item": incoming[0]["from"] });
    let outgoing = client.request("callHierarchy/outgoingCalls", main);
    assert_eq!(calls(&outgoing, "to"), [("f".into(), 1), ("p".into(), 1)]);

    // The instance tree: `top` instantiates `sub` twice.
    let items = client.request("textDocument/prepareCallHierarchy", at(&sub, 0, 8));
    assert_eq!(items[0]["name"], "sub");
    assert_eq!(items[0]["detail"], "architecture rtl");
    let incoming = client.request("callHierarchy/incomingCalls", json!({ "item": items[0] }));
    assert_eq!(calls(&incoming, "from"), [("top".into(), 2)]);
    let from = json!({ "item": incoming[0]["from"] });
    let outgoing = client.request("callHierarchy/outgoingCalls", from);
    assert_eq!(calls(&outgoing, "to"), [("sub".into(), 2)]);
    assert_eq!(outgoing[0]["to"]["uri"], sub);
}
