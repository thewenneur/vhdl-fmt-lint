//! Checks of the documentation model, the diagrams and the pages.

use vfl_config::{DocComments, DocConfig, DocFormat, MarkdownDiagrams};
use vfl_doc::model::{self, Direction, FileInput, InstanceKind};
use vfl_doc::{Design, OutputFile, hierarchy, svg, text};
use vfl_syntax::{Standard, SyntaxNode};

fn design_with(files: &[(&str, &str)], config: &DocConfig) -> Design {
    let trees: Vec<SyntaxNode> = files
        .iter()
        .map(|(_, text)| vfl_syntax::parse(text, Standard::Vhdl2008).syntax())
        .collect();
    let inputs: Vec<FileInput> = files
        .iter()
        .zip(&trees)
        .map(|((path, text), root)| FileInput {
            root,
            text,
            path: (*path).to_owned(),
            library: "work".into(),
        })
        .collect();
    model::build(&inputs, config, "Test".into())
}

fn design(files: &[(&str, &str)]) -> Design {
    design_with(files, &DocConfig::default())
}

const COUNTER: &str = "\
library ieee;
use ieee.std_logic_1164.all;

-- License header, not documentation.

--! A simple up counter.
--!
--! Wraps around at the maximum value.
entity counter is
  generic (
    WIDTH : positive := 8 --! Counter width in bits
  );
  port (
    clk, rst : in  std_logic;   --! Clock and synchronous reset
    en       : in  std_logic;
    -- Not a doc comment: the file uses --! ones.
    count    : out std_logic_vector(WIDTH - 1 downto 0);
    bidir    : inout std_logic
  );
end entity counter;

--! Counts while enabled.
architecture rtl of counter is
  type state_t is (IDLE, RUN, DONE);
  signal state : state_t := IDLE; --! The state register
  signal value : natural range 0 to 255;
  constant MAX : natural := 10;
  function f(a : integer) return integer is
  begin
    return a;
  end function;
begin
  --! Counts and steps the state machine.
  counting : process (clk) is
  begin
    if rising_edge(clk) then
      if rst = '1' then
        value <= 0;
        state <= IDLE;
      else
        case state is
          when IDLE =>
            if en = '1' then
              state <= RUN;
            end if;
          when RUN =>
            value <= value + 1;
            if value = MAX then
              state <= DONE;
            elsif en = '0' then
              state <= IDLE;
            end if;
          when others =>
            state <= IDLE;
        end case;
      end if;
    end if;
  end process counting;
end architecture rtl;
";

const TOP: &str = "\
entity top is
  port (clk : in bit; q : out bit);
end entity;

architecture str of top is
  component counter is
    port (clk : in bit);
  end component;
begin
  -- The main counter.
  u_cnt : entity work.counter(rtl) generic map (WIDTH => 4) port map (clk => clk, en => '1');
  gen : for i in 0 to 1 generate
    u_c : counter port map (clk);
  end generate;
  u_prim : BUFG port map (I => clk, O => q);
end architecture;

entity tb is
end entity;

architecture sim of tb is
begin
  dut : entity work.top port map (clk => '0', q => open);
end architecture;
";

#[test]
fn reads_entities_ports_and_doc_comments() {
    let d = design(&[("rtl/counter.vhd", COUNTER)]);
    assert_eq!(d.entities.len(), 1);
    let e = &d.entities[0];
    assert_eq!(e.name, "counter");
    assert_eq!(e.line, 9);
    assert_eq!(
        e.description,
        "A simple up counter.\n\nWraps around at the maximum value."
    );
    assert_eq!(e.generics.len(), 1);
    assert_eq!(e.generics[0].ty, "positive");
    assert_eq!(e.generics[0].default.as_deref(), Some("8"));
    assert_eq!(e.generics[0].description, "Counter width in bits");
    let names: Vec<&str> = e.ports.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["clk", "rst", "en", "count", "bidir"]);
    assert_eq!(e.ports[1].description, "Clock and synchronous reset");
    assert_eq!(e.ports[2].description, "");
    assert_eq!(e.ports[3].description, "");
    assert_eq!(e.ports[3].ty, "std_logic_vector(WIDTH - 1 downto 0)");
    assert_eq!(e.ports[3].direction(), Direction::Out);
    assert_eq!(e.ports[4].direction(), Direction::InOut);

    let arch = &e.architectures[0];
    assert_eq!(arch.description, "Counts while enabled.");
    assert_eq!(arch.signals[0].description, "The state register");
    assert_eq!(arch.signals[0].value.as_deref(), Some("IDLE"));
    assert_eq!(arch.constants[0].name, "MAX");
    assert_eq!(arch.types[0].definition, "(IDLE, RUN, DONE)");
    assert_eq!(
        arch.subprograms[0].signature,
        "function f(a : integer) return integer"
    );
    let p = &arch.processes[0];
    assert_eq!(p.label.as_deref(), Some("counting"));
    assert_eq!(p.sensitivity, ["clk"]);
    assert_eq!(p.clock.as_deref(), Some("clk"));
    assert_eq!(p.drives, ["value", "state"]);
    assert_eq!(p.description, "Counts and steps the state machine.");
}

#[test]
fn comment_modes() {
    let mut config = DocConfig {
        comments: DocComments::All,
        ..DocConfig::default()
    };
    let d = design_with(&[("c.vhd", COUNTER)], &config);
    let e = &d.entities[0];
    assert_eq!(
        e.ports[3].description,
        "Not a doc comment: the file uses --! ones."
    );
    // `--!` comments are comments too; their marker is dropped.
    assert_eq!(e.ports[1].description, "Clock and synchronous reset");

    config.comments = DocComments::None;
    let d = design_with(&[("c.vhd", COUNTER)], &config);
    assert_eq!(d.entities[0].description, "");

    // Without any `--!` in the file, `auto` takes every adjacent comment.
    let plain = "-- A register.\nentity r is\n  port (d : in bit -- data in\n  );\nend;\n";
    let d = design(&[("r.vhd", plain)]);
    assert_eq!(d.entities[0].description, "A register.");
    assert_eq!(d.entities[0].ports[0].description, "data in");
}

#[test]
fn detects_one_process_state_machines() {
    let d = design(&[("c.vhd", COUNTER)]);
    let fsm = &d.entities[0].architectures[0].fsms[0];
    assert_eq!(fsm.signal, "state");
    assert_eq!(fsm.type_name, "state_t");
    assert_eq!(fsm.initial.as_deref(), Some("IDLE"));
    assert_eq!(fsm.next, None);
    assert_eq!(fsm.process.as_deref(), Some("counting"));
    assert_eq!(fsm.description, "The state register");
    let transitions: Vec<(&str, &str, Option<&str>)> = fsm
        .transitions
        .iter()
        .map(|t| (t.from.as_str(), t.to.as_str(), t.condition.as_deref()))
        .collect();
    assert_eq!(
        transitions,
        [
            ("IDLE", "RUN", Some("en = '1'")),
            ("RUN", "DONE", Some("value = MAX")),
            ("RUN", "IDLE", Some("en = '0'")),
            ("DONE", "IDLE", None),
        ]
    );
}

#[test]
fn detects_two_process_and_record_state_machines() {
    let text = "\
package types is
  type mode_t is (OFF, ON_, BLINK);
end package;

use work.types.all;
entity fsm2 is
  port (clk, go : in bit);
end;

architecture rtl of fsm2 is
  type state_t is (S0, S1);
  signal state, state_next : state_t;
  type engine_t is record
    mode : mode_t;
    count : natural;
  end record;
  signal engine, engine_nxt : engine_t;
begin
  comb : process (all) begin
    state_next <= state;
    case state is
      when S0 => if go = '1' then state_next <= S1; end if;
      when S1 => state_next <= S0;
    end case;
  end process;

  regs : process (clk) begin
    if rising_edge(clk) then
      state <= state_next;
      engine <= engine_nxt;
    end if;
  end process;

  eng : process (all) begin
    engine_nxt <= engine;
    case engine.mode is
      when OFF => engine_nxt.mode <= ON_;
      when ON_ | BLINK =>
        case go is
          when '1' => engine_nxt.mode <= BLINK;
          when others => engine_nxt.mode <= OFF;
        end case;
    end case;
  end process;
end;
";
    let d = design(&[("f.vhd", text)]);
    let fsms = &d.entities[0].architectures[0].fsms;
    assert_eq!(fsms.len(), 2);
    assert_eq!(fsms[0].signal, "state");
    assert_eq!(fsms[0].next.as_deref(), Some("state_next"));
    // `state_next <= state` is no transition.
    assert_eq!(fsms[0].transitions.len(), 2);
    assert_eq!(
        fsms[0].transitions[0].condition.as_deref(),
        Some("go = '1'")
    );
    assert_eq!(fsms[1].signal, "engine.mode");
    assert_eq!(fsms[1].next.as_deref(), Some("engine_nxt.mode"));
    assert_eq!(fsms[1].states, ["OFF", "ON_", "BLINK"]);
    let blink = fsms[1]
        .transitions
        .iter()
        .find(|t| t.from == "ON_" && t.to == "BLINK")
        .unwrap();
    assert_eq!(blink.condition.as_deref(), Some("go = '1'"));
    assert!(svg::fsm(&fsms[1]).contains(">BLINK</text>"));
}

#[test]
fn resolves_the_hierarchy() {
    let d = design(&[("c.vhd", COUNTER), ("top.vhd", TOP)]);
    let top = d.entities.iter().position(|e| e.name == "top").unwrap();
    let instances = &d.entities[top].architectures[0].instances;
    assert_eq!(instances[0].kind, InstanceKind::Entity);
    assert_eq!(instances[0].library.as_deref(), Some("work"));
    assert_eq!(instances[0].architecture.as_deref(), Some("rtl"));
    assert_eq!(
        instances[0].generic_map,
        [("WIDTH".to_owned(), "4".to_owned())]
    );
    assert_eq!(instances[0].description, "The main counter.");
    assert_eq!(instances[1].path(), "gen.u_c");
    assert_eq!(instances[1].port_map, [(String::new(), "clk".to_owned())]);

    let mut roots = hierarchy::build(&d);
    assert_eq!(roots.len(), 1, "only the testbench is a root");
    assert_eq!(roots[0].unit, "tb");
    let dut = &roots[0].children[0];
    assert_eq!(dut.label.as_deref(), Some("dut"));
    let labels: Vec<_> = dut
        .children
        .iter()
        .map(|c| c.label.clone().unwrap())
        .collect();
    assert_eq!(labels, ["u_cnt", "gen.u_c", "u_prim"]);
    assert!(
        dut.children[2].entity.is_none(),
        "BUFG is not in the design"
    );
    assert_eq!(dut.children[0].architecture.as_deref(), Some("rtl"));

    // A second tree with the same subtrees draws them once.
    roots.push(roots[0].clone());
    hierarchy::collapse_repeats(&mut roots);
    assert!(!roots[0].children[0].repeated);
    assert!(roots[1].repeated);
    assert!(roots[1].children.is_empty());

    let mermaid = text::hierarchy_mermaid(&roots[..1]);
    assert!(mermaid.starts_with("flowchart TD\n"), "{mermaid}");
    assert!(mermaid.contains("n1 -->|\"u_cnt\"| n2"), "{mermaid}");
    let dot = text::hierarchy_dot(&roots[..1]);
    assert!(
        dot.contains("[label=\"BUFG (external)\", style=\"rounded,dashed\"]"),
        "{dot}"
    );
}

#[test]
fn symbol_puts_inputs_left_and_outputs_right() {
    let d = design(&[("c.vhd", COUNTER)]);
    let options = svg::SymbolOptions {
        port_types: true,
        generics: true,
    };
    let symbol = svg::symbol(&d.entities[0], options);
    assert!(symbol.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
    assert!(symbol.contains("text-anchor=\"start\">clk</text>"));
    assert!(symbol.contains("text-anchor=\"end\">count</text>"));
    assert!(symbol.contains("WIDTH : positive := 8"));
    assert!(symbol.contains("std_logic_vector(WIDTH - 1 downto 0)"));
    let bare = svg::symbol(
        &d.entities[0],
        svg::SymbolOptions {
            port_types: false,
            generics: false,
        },
    );
    assert!(!bare.contains("WIDTH : positive"));
    assert!(!bare.contains("std_logic_vector"));
}

#[test]
fn text_diagrams_escape_labels() {
    let d = design(&[("c.vhd", COUNTER)]);
    let mut fsm = d.entities[0].architectures[0].fsms[0].clone();
    fsm.transitions[0].condition = Some("x = \"01\"; y : z".into());
    let mermaid = text::fsm_mermaid(&fsm);
    assert!(mermaid.contains("[*] --> IDLE"), "{mermaid}");
    assert!(
        mermaid.contains("IDLE --> RUN : x = #quot;01#quot;#59; y #58; z"),
        "{mermaid}"
    );
    let dot = text::fsm_dot(&fsm);
    assert!(dot.contains("\"IDLE\" [shape=doublecircle];"), "{dot}");
    assert!(dot.contains("[label=\"x = \\\"01\\\"; y : z\"]"), "{dot}");
}

fn file<'a>(files: &'a [OutputFile], path: &str) -> &'a str {
    &files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| {
            panic!(
                "no {path} in {:?}",
                files.iter().map(|f| &f.path).collect::<Vec<_>>()
            )
        })
        .contents
}

#[test]
fn writes_markdown_pages() {
    let config = DocConfig {
        format: DocFormat::Markdown,
        ..DocConfig::default()
    };
    let d = design(&[("rtl/counter.vhd", COUNTER), ("top.vhd", TOP)]);
    let files = vfl_doc::generate(&d, &config);
    let index = file(&files, "index.md");
    assert!(
        index.contains("![Design hierarchy](hierarchy.svg)"),
        "{index}"
    );
    assert!(
        index.contains("| [`counter`](work.counter.md) | `work` | A simple up counter. |"),
        "{index}"
    );
    assert!(
        index.contains("| [`tb`](work.tb.md) | `work` | Testbench. |"),
        "{index}"
    );
    let counter = file(&files, "work.counter.md");
    assert!(
        counter.contains("Library `work` · `rtl/counter.vhd:9`"),
        "{counter}"
    );
    assert!(
        counter.contains("![Symbol of counter](work.counter.symbol.svg)"),
        "{counter}"
    );
    assert!(
        counter.contains("| `clk` | in | `std_logic` | Clock and synchronous reset |"),
        "{counter}"
    );
    assert!(
        counter.contains("- [`top(str)`](work.top.md#arch-str) as `u_cnt`"),
        "{counter}"
    );
    assert!(
        counter.contains("| `IDLE` | `RUN` | `en = '1'` |"),
        "{counter}"
    );
    assert!(counter.contains("![State machine state](work.counter.rtl.fsm-state.svg)"));
    file(&files, "work.counter.rtl.fsm-state.svg");
    // Columns nothing fills are left out.
    assert!(counter.contains("| Name | Type | Default | Description |"));
    assert!(counter.contains("| Process | Sensitivity | Clock | Drives | Description |"));
    let top = file(&files, "work.top.md");
    assert!(
        top.contains("| Instance | Unit | Kind | Description |"),
        "{top}"
    );
    assert!(
        top.contains("| `u_prim` | `BUFG` | component |  |"),
        "{top}"
    );

    let mermaid = DocConfig {
        diagrams: vfl_config::DiagramConfig {
            markdown: MarkdownDiagrams::Mermaid,
            ..config.diagrams.clone()
        },
        ..config
    };
    let files = vfl_doc::generate(&d, &mermaid);
    assert!(file(&files, "index.md").contains("```mermaid\nflowchart TD\n"));
    assert!(file(&files, "work.counter.md").contains("```mermaid\nstateDiagram-v2\n"));
    assert!(!files.iter().any(|f| f.path == "hierarchy.svg"));
}

#[test]
fn writes_html_pages() {
    let d = design(&[("rtl/counter.vhd", COUNTER), ("top.vhd", TOP)]);
    let files = vfl_doc::generate(&d, &DocConfig::default());
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "index.html",
            "work.counter.html",
            "work.tb.html",
            "work.top.html"
        ]
    );
    let counter = file(&files, "work.counter.html");
    assert!(counter.starts_with("<!DOCTYPE html>"));
    assert!(counter.contains("<li class=\"current\"><a href=\"work.counter.html\""));
    assert!(counter.contains("<div class=\"diagram\"><svg"));
    assert!(
        counter.contains("<p>A simple up counter.</p><p>Wraps around at the maximum value.</p>")
    );
    assert!(counter.contains("<td><code>en = &#39;1&#39;</code></td>"));
    // Hierarchy boxes link to the entity pages.
    assert!(file(&files, "index.html").contains("<a href=\"work.counter.html\"><rect"));
}

#[test]
fn options_leave_sections_out() {
    let config = DocConfig {
        format: DocFormat::Markdown,
        testbenches: false,
        signals: false,
        processes: false,
        fsm: false,
        diagrams: vfl_config::DiagramConfig {
            symbol: false,
            hierarchy: false,
            ..Default::default()
        },
        ..DocConfig::default()
    };
    let d = design_with(&[("c.vhd", COUNTER), ("top.vhd", TOP)], &config);
    assert!(!d.entities.iter().any(|e| e.name == "tb"));
    let files = vfl_doc::generate(&d, &config);
    let counter = file(&files, "work.counter.md");
    assert!(!counter.contains("### Signals"));
    assert!(!counter.contains("### Processes"));
    assert!(!counter.contains("State machine"));
    assert!(!counter.contains(".svg"));
    assert!(!files.iter().any(|f| f.path.ends_with(".svg")));
}

#[test]
fn units_of_the_same_name_get_their_own_pages() {
    let a = "--! First.\npackage p is\nend package;\n";
    let b = "--! Second.\npackage p is\nend package;\n";
    let d = design(&[
        ("a.vhd", a),
        ("b.vhd", b),
        ("i.vhd", "entity index is end;"),
    ]);
    let pages: Vec<&str> = d.packages.iter().map(|p| p.page.as_str()).collect();
    assert_eq!(pages, ["work.p", "work.p-2"]);
    assert_eq!(d.entities[0].page, "work.index");
}

#[test]
fn loads_a_project_and_skips_excluded_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("rtl")).unwrap();
    std::fs::create_dir_all(dir.path().join("tb")).unwrap();
    std::fs::write(dir.path().join("vfl.toml"), "[doc]\nexclude = [\"tb\"]\n").unwrap();
    std::fs::write(dir.path().join("rtl/counter.vhd"), COUNTER).unwrap();
    std::fs::write(dir.path().join("tb/top.vhd"), TOP).unwrap();
    let root = vfl_project::find_root(dir.path());
    let project = vfl_project::Project::load(root).unwrap();
    let d = vfl_doc::load(&project, &project.config.doc).unwrap();
    let names: Vec<&str> = d.entities.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["counter"]);
    assert_eq!(d.entities[0].file, "rtl/counter.vhd");
    assert_eq!(d.title, dir.path().file_name().unwrap().to_string_lossy());
}
