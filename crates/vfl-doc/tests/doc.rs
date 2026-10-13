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
            block: false,
            port_map: false,
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

fn transitions(fsm: &vfl_doc::Fsm) -> Vec<String> {
    fsm.transitions
        .iter()
        .map(|t| match &t.condition {
            Some(c) => format!("{} -> {} : {c}", t.from, t.to),
            None => format!("{} -> {}", t.from, t.to),
        })
        .collect()
}

#[test]
fn detects_state_machines_in_every_process_style() {
    let text = "\
entity a is port (clk, rst, go : in bit); end;
architecture rtl of a is
  type st is (IDLE, RUN, DONE);
  signal t, s, s_next, u, u_next, w : st;
begin
  -- Asynchronous reset.
  p1 : process (clk, rst) begin
    if rst = '1' then
      t <= IDLE;
    elsif rising_edge(clk) then
      case t is
        when IDLE => if go = '1' then t <= RUN; end if;
        when RUN => t <= DONE;
        when DONE => t <= IDLE;
      end case;
    end if;
  end process;
  -- Asynchronous reset register and combinational next state.
  reg : process (clk, rst) begin
    if rst = '1' then s <= IDLE; elsif rising_edge(clk) then s <= s_next; end if;
  end process;
  comb : process (s, go) begin
    s_next <= s;
    case s is
      when IDLE => s_next <= RUN when go = '1' else IDLE;
      when RUN => s_next <= DONE;
      when others => s_next <= IDLE;
    end case;
  end process;
  -- Concurrent selected assignment.
  with u select u_next <=
    RUN when IDLE,
    DONE when RUN,
    IDLE when others;
  -- A process with a wait statement.
  pw : process begin
    wait until rising_edge(clk);
    case w is
      when IDLE => w <= RUN;
      when others => w <= IDLE;
    end case;
  end process;
end;

architecture conc of a is
  type st is (IDLE, RUN, DONE);
  signal v, v_next : st;
begin
  v <= IDLE when rst = '1' else v_next when rising_edge(clk);
  v_next <= RUN when v = IDLE and go = '1' else DONE when v = RUN else IDLE;
end;
";
    let d = design(&[("a.vhd", text)]);
    let fsms = &d.entities[0].architectures[0].fsms;
    let by = |name: &str| fsms.iter().find(|f| f.signal == name).unwrap();
    let t = by("t");
    assert_eq!(t.initial.as_deref(), Some("IDLE"));
    assert_eq!(
        transitions(t),
        ["IDLE -> RUN : go = '1'", "RUN -> DONE", "DONE -> IDLE"]
    );
    let s = by("s");
    assert_eq!(s.initial.as_deref(), Some("IDLE"));
    assert_eq!(s.next.as_deref(), Some("s_next"));
    assert_eq!(s.process.as_deref(), Some("comb"));
    assert_eq!(
        transitions(s),
        [
            "IDLE -> RUN : go = '1'",
            "IDLE -> IDLE : not (go = '1')",
            "RUN -> DONE",
            "DONE -> IDLE"
        ]
    );
    let u = by("u");
    assert_eq!(u.next.as_deref(), Some("u_next"));
    assert_eq!(
        transitions(u),
        ["IDLE -> RUN", "RUN -> DONE", "DONE -> IDLE"]
    );
    assert_eq!(
        transitions(by("w")),
        ["IDLE -> RUN", "RUN -> IDLE", "DONE -> IDLE"]
    );

    let v = &d.entities[0].architectures[1].fsms[0];
    assert_eq!(v.signal, "v");
    assert_eq!(v.initial.as_deref(), Some("IDLE"));
    assert_eq!(
        transitions(v),
        [
            "IDLE -> RUN : go = '1'",
            "RUN -> DONE",
            "DONE -> IDLE",
            "IDLE -> IDLE : else"
        ]
    );
}

#[test]
fn block_diagram_wires_instances_by_signals() {
    let text = "\
entity stage is
  port (clk : in bit; d : in bit; q : out bit);
end;
architecture rtl of stage is begin end;

entity pipe is
  port (clk, din : in bit; dout : out bit);
end;
architecture str of pipe is
  signal a, b : bit;
begin
  s1 : entity work.stage port map (clk => clk, d => din, q => a);
  s2 : entity work.stage port map (clk, a, b);
  s3 : entity work.stage port map (clk => clk, d => b, q => dout);
  u_ext : vendor_buf port map (i => b, o => open);
end;
";
    let d = design(&[("p.vhd", text)]);
    let pipe = d.entities.iter().position(|e| e.name == "pipe").unwrap();
    let svg = vfl_doc::block::block(&d, pipe, 0, None).unwrap();
    // One block per instance, and a terminal per port.
    for name in ["s1", "s2", "s3", "u_ext", "din", "dout"] {
        assert!(
            svg.contains(&format!(">{name}</text>")),
            "{name} missing: {svg}"
        );
    }
    // `a` and `b` run from one column to the next, as wires named on hover.
    assert!(
        svg.contains("<g><title>a</title><path class=\"vfl-wire\""),
        "{svg}"
    );
    assert!(
        svg.contains("<g><title>b</title><path class=\"vfl-wire\""),
        "{svg}"
    );
    // `clk` reaches the later columns through labelled stubs.
    assert!(svg.contains(">clk</text>"));
    // Nothing to draw without instances.
    let stage = d.entities.iter().position(|e| e.name == "stage").unwrap();
    assert!(vfl_doc::block::block(&d, stage, 0, None).is_none());
}

#[test]
fn port_map_shows_actuals_and_generics() {
    let d = design(&[("rtl/counter.vhd", COUNTER), ("top.vhd", TOP)]);
    let top = d.entities.iter().position(|e| e.name == "top").unwrap();
    let svg = vfl_doc::block::port_map(&d, top, 0, 0).unwrap();
    // The instance, its unit, the mapped generic and the actuals.
    assert!(svg.contains(">u_cnt<"), "{svg}");
    assert!(svg.contains(">counter(rtl)<"), "{svg}");
    assert!(svg.contains(">WIDTH =&gt; 4<"), "{svg}");
    assert!(svg.contains(r#"class="vfl-actual">clk<"#), "{svg}");
    assert!(svg.contains(r#"class="vfl-actual">&#39;1&#39;<"#), "{svg}");
    // A positional association to a component, and an unknown primitive.
    let svg = vfl_doc::block::port_map(&d, top, 0, 1).unwrap();
    assert!(svg.contains(">gen.u_c<"), "{svg}");
    assert!(svg.contains(r#"class="vfl-actual">clk<"#), "{svg}");
    let svg = vfl_doc::block::port_map(&d, top, 0, 2).unwrap();
    assert!(svg.contains(">I<") && svg.contains(">O<"), "{svg}");
    assert!(vfl_doc::block::port_map(&d, top, 0, 3).is_none());
}

#[test]
fn index_draws_the_top_level_under_its_testbench() {
    let config = DocConfig {
        format: DocFormat::Markdown,
        ..DocConfig::default()
    };
    let d = design(&[("rtl/counter.vhd", COUNTER), ("top.vhd", TOP)]);
    let files = vfl_doc::generate(&d, &config);
    let index = file(&files, "index.md");
    // `tb` is the root; the top level is the design it tests.
    assert!(index.contains("### [`top`](work.top.md) (str)"), "{index}");
    assert!(index.contains("](top.work.top.block.svg)"), "{index}");
    let top = file(&files, "work.top.md");
    assert!(top.contains("### Port maps"), "{top}");
    assert!(top.contains("](work.top.str.map-u_cnt.svg)"), "{top}");
    assert!(top.contains("](work.top.str.map-gen_u_c.svg)"), "{top}");
}

#[test]
fn writes_asciidoc_pages() {
    let config = DocConfig {
        format: DocFormat::Asciidoc,
        ..DocConfig::default()
    };
    let d = design(&[("rtl/counter.vhd", COUNTER), ("top.vhd", TOP)]);
    let files = vfl_doc::generate(&d, &config);
    let index = file(&files, "index.adoc");
    assert!(index.starts_with("= "), "{index}");
    assert!(
        index.contains("image::hierarchy.svg[\"Design hierarchy\"]"),
        "{index}"
    );
    assert!(
        index.contains(
            "|xref:work.counter.adoc[``++counter++``] |``++work++`` |A simple up counter."
        ),
        "{index}"
    );
    let counter = file(&files, "work.counter.adoc");
    assert!(counter.contains("[[ports]]\n== Ports"), "{counter}");
    assert!(counter.contains("|===\n"), "{counter}");
    file(&files, "work.counter.symbol.svg");
}

const SYNC: &str = "\
library ieee;
use ieee.std_logic_1164.all;
use work.regs_pkg.all;

entity sync is
  port (clk_a, clk_b, rst_n, srst : in std_logic; d : in std_logic; q : out std_logic);
end;
architecture rtl of sync is
  signal toggle, local : std_logic;
  signal stages : std_logic_vector(2 downto 0);
  alias meta : std_logic is stages(0);
  alias stable : std_logic_vector(1 downto 0) is stages(2 downto 1);
begin
  source : process (clk_a, rst_n) begin
    if rst_n = '0' then
      toggle <= '0';
    elsif rising_edge(clk_a) then
      toggle <= toggle xor d;
    end if;
  end process;
  destination : process (clk_b) begin
    if falling_edge(clk_b) then
      if srst = '1' then
        stages <= (others => '0');
      else
        stages <= stable(0) & meta & toggle;
      end if;
    end if;
  end process;
  local <= stages(2);
  q <= local;
end;
";

const REGS_PKG: &str = "\
package regs_pkg is
  constant WIDTH : natural := 6;
  type ctrl_t is record
    enable : bit;
    mode : bit_vector(1 downto 0);
    count : integer range 0 to 2 ** WIDTH - 1;
    data : bit_vector(WIDTH * 2 - 1 downto 0);
  end record;
end package;
";

#[test]
fn records_processes_clocks_resets_and_assignments() {
    let d = design(&[("sync.vhd", SYNC), ("regs_pkg.vhd", REGS_PKG)]);
    let e = d.entities.iter().find(|e| e.name == "sync").unwrap();
    let a = &e.architectures[0];
    let source = &a.processes[0];
    assert_eq!(source.clock.as_deref(), Some("clk_a"));
    assert!(!source.falling);
    let reset = source.reset.as_ref().unwrap();
    assert_eq!(reset.signal, "rst_n");
    assert!(reset.asynchronous);
    assert_eq!(reset.active_high, Some(false));
    let destination = &a.processes[1];
    assert_eq!(destination.clock.as_deref(), Some("clk_b"));
    assert!(destination.falling);
    let reset = destination.reset.as_ref().unwrap();
    assert_eq!(reset.signal, "srst");
    assert!(!reset.asynchronous);
    assert_eq!(reset.active_high, Some(true));
    // Aliases resolve to the signal they name.
    assert!(destination.reads.iter().any(|r| r == "toggle"));
    assert_eq!(
        a.resolve_aliases(&["meta".into(), "toggle".into()]),
        ["stages", "toggle"]
    );
    assert_eq!(a.assignments.len(), 2);
    assert_eq!(a.assignments[0].target, "local");
    assert_eq!(a.assignments[0].reads, ["stages"]);
    assert_eq!(e.uses.len(), 2, "{:?}", e.uses);
}

#[test]
fn dataflow_wires_processes_and_assignments() {
    let d = design(&[("sync.vhd", SYNC)]);
    let svg = vfl_doc::block::dataflow(&d, 0, 0, None).unwrap();
    for text in [
        ">source</text>",
        ">destination</text>",
        ">local &lt;=</text>",
        ">q &lt;=</text>",
        "clk_a ↑, rst_n async",
        "clk_b ↓, srst sync",
    ] {
        assert!(svg.contains(text), "{text} missing: {svg}");
    }
    // Clocks and resets are in the subtitles, not terminals.
    assert!(!svg.contains(">clk_a</text>"), "{svg}");
    // An architecture with nothing to draw.
    let d = design(&[(
        "c.vhd",
        "entity e is end; architecture a of e is begin end;",
    )]);
    assert!(vfl_doc::block::dataflow(&d, 0, 0, None).is_none());
}

#[test]
fn clock_domains_find_the_crossings() {
    let d = design(&[("sync.vhd", SYNC)]);
    let (svg, domains) = vfl_doc::clocks::diagram(&d, 0, 0).unwrap();
    let names: Vec<&str> = domains.domains.iter().map(|(c, _)| c.as_str()).collect();
    assert_eq!(names, ["clk_a", "clk_b"]);
    assert_eq!(domains.domains[1].1, ["destination"]);
    assert_eq!(
        domains.crossings,
        [vfl_doc::clocks::Crossing {
            signal: "toggle".into(),
            from: "clk_a".into(),
            to: "clk_b".into(),
            reader: "destination".into(),
        }]
    );
    assert!(svg.contains("vfl-cross"), "{svg}");
    // One clock: no diagram.
    let d = design(&[("c.vhd", COUNTER)]);
    assert!(vfl_doc::clocks::diagram(&d, 0, 0).is_none());
}

#[test]
fn reset_tree_follows_ports_into_instances() {
    let top = "\
entity top is
  port (clk, rst_n, d : in bit; q : out bit);
end;
architecture str of top is
  signal rst : bit;
begin
  rst <= not rst_n;
  u_sync : entity work.sync port map (clk_a => clk, clk_b => clk, rst_n => rst_n,
                                      srst => rst, d => d, q => q);
end;
";
    let d = design(&[("sync.vhd", SYNC), ("top.vhd", top)]);
    let t = d.entities.iter().position(|e| e.name == "top").unwrap();
    let svg = vfl_doc::relations::resets(&d, t, 0, None).unwrap();
    for text in [
        ">rst_n</text>",
        ">u_sync</text>",
        ">source</text>",
        "async, active low",
        ">destination</text>",
        "sync, active high",
    ] {
        assert!(svg.contains(text), "{text} missing: {svg}");
    }
}

#[test]
fn dependencies_link_units_to_packages() {
    let d = design(&[("sync.vhd", SYNC), ("regs_pkg.vhd", REGS_PKG)]);
    let svg = vfl_doc::relations::dependencies(&d, None).unwrap();
    assert!(
        svg.contains(">sync</text>") && svg.contains(">regs_pkg</text>"),
        "{svg}"
    );
    // `ieee` is outside the project.
    assert!(!svg.contains(">std_logic_1164</text>"), "{svg}");
    assert!(svg.contains("vfl-edge"), "{svg}");
    let d = design(&[("c.vhd", COUNTER)]);
    assert!(vfl_doc::relations::dependencies(&d, None).is_none());
}

#[test]
fn wavedrom_parses_json5_and_draws_lanes() {
    let value = vfl_doc::wave::parse(
        "{ signal: [ { name: 'clk', wave: 'p...' }, // a comment\n {}, ], head: {text: \"t\"}, }",
    )
    .unwrap();
    let vfl_doc::wave::Value::Object(fields) = &value else {
        panic!("{value:?}");
    };
    assert_eq!(fields[0].0, "signal");
    let svg = vfl_doc::wave::draw(
        r#"{ signal: [
            { name: "clk", wave: "p..." },
            ["bus", { name: "data", wave: "x=.=", data: ["a", "b"] }],
            { name: "en", wave: "01|0" }
        ], foot: { text: "end" } }"#,
    )
    .unwrap();
    for text in [
        ">clk</text>",
        ">bus</text>",
        ">data</text>",
        ">a</text>",
        ">b</text>",
        ">end</text>",
    ] {
        assert!(svg.contains(text), "{text} missing: {svg}");
    }
    assert!(svg.contains("vfl-wave-x"), "{svg}");
    assert!(vfl_doc::wave::draw("{ signal: [ { wave: ").is_err());
    assert!(vfl_doc::wave::draw("{ nothing: 1 }").is_err());
}

#[test]
fn wavedrom_blocks_in_doc_comments_become_diagrams() {
    let text = "\
--! A strobe.
--! ```wavedrom
--! { signal: [ { name: \"stb\", wave: \"010\" } ] }
--! ```
entity strobe is
  port (stb : out bit);
end;
";
    let config = DocConfig {
        format: DocFormat::Markdown,
        ..DocConfig::default()
    };
    let d = design_with(&[("s.vhd", text)], &config);
    let files = vfl_doc::generate(&d, &config);
    let page = file(&files, "work.strobe.md");
    assert!(!page.contains("```wavedrom"), "{page}");
    let wave = files
        .iter()
        .find(|f| f.path.starts_with("wave-") && f.path.ends_with(".svg"))
        .expect("a wave svg");
    assert!(page.contains(&format!("]({})", wave.path)), "{page}");
    // Off: the block stays as code.
    let mut config = config;
    config.diagrams.waveforms = false;
    let files = vfl_doc::generate(&d, &config);
    assert!(file(&files, "work.strobe.md").contains("```wavedrom"));
}

#[test]
fn bit_fields_of_records_and_aliased_slices() {
    let d = design(&[("sync.vhd", SYNC), ("regs_pkg.vhd", REGS_PKG)]);
    let p = &d.packages[0];
    let packages = vfl_doc::fields::Scope::packages(&d);
    let scope = vfl_doc::fields::Scope::new(&packages, &d, None, &p.constants, &p.types);
    assert_eq!(scope.integer("2 ** WIDTH - 1"), Some(63));
    assert_eq!(scope.integer("(WIDTH + 2) / 3 mod 2"), Some(0));
    assert_eq!(scope.bits("bit_vector(WIDTH * 2 - 1 downto 0)"), Some(12));
    assert_eq!(scope.bits("integer range 0 to 2 ** WIDTH - 1"), Some(6));
    assert_eq!(scope.bits("bit"), Some(1));
    assert_eq!(scope.bits("ctrl_t"), Some(21));
    assert_eq!(scope.integer("WIDTH / 0"), None);
    let svg = vfl_doc::fields::record(&scope, &p.types[0]).unwrap();
    assert!(svg.contains("ctrl_t : 21 bits"), "{svg}");
    assert!(svg.contains(">12 bits</text>"), "{svg}");

    let e = d.entities.iter().position(|e| e.name == "sync").unwrap();
    let a = &d.entities[e].architectures[0];
    let scope = vfl_doc::fields::Scope::new(&packages, &d, Some(e), &a.constants, &a.types);
    let maps = vfl_doc::fields::aliases(&scope, &a.signals, &a.aliases);
    assert_eq!(maps.len(), 1);
    assert_eq!(maps[0].0, "stages");
    assert!(maps[0].1.contains(">stable</text>") && maps[0].1.contains(">meta</text>"));
}
