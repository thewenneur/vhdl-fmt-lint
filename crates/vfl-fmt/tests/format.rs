//! Formatter tests: snapshots of whole files, idempotency over every test
//! file in the workspace, and one test per configuration option.

use std::path::Path;

use vfl_config::{Config, FormatConfig};
use vfl_fmt::{FormatError, format};
use vfl_syntax::Standard;

/// Formats `text` and checks that formatting the result changes nothing.
fn fmt_with(text: &str, config: &FormatConfig) -> String {
    let out = format(text, config, Standard::Vhdl2019).unwrap_or_else(|e| panic!("{e}"));
    let again = format(&out, config, Standard::Vhdl2019).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(again, out, "formatting is not idempotent");
    out
}

fn fmt(text: &str) -> String {
    fmt_with(text, &FormatConfig::default())
}

/// A format config from the body of a `[format]` table.
fn config(toml: &str) -> FormatConfig {
    Config::from_toml_str(&format!("[format]\n{toml}"))
        .unwrap()
        .format
}

#[test]
fn snapshots() {
    insta::glob!("data/*.vhd", |path| {
        let text = std::fs::read_to_string(path).unwrap();
        insta::assert_snapshot!(fmt(&text));
    });
}

#[test]
fn parser_test_files_are_stable() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../vfl-syntax/tests/data/ok");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        for config in [
            FormatConfig::default(),
            config(
                "keyword_case = \"upper\"\nidentifier_case = \"upper\"\nuse_tabs = true\nindent = 4",
            ),
            config("end_label = \"add\"\nend_keyword = \"add\"\nmax_width = 40"),
            config(
                "end_label = \"remove\"\nend_keyword = \"remove\"\ninterface_list = \"preserve\"",
            ),
            config("[format.align]\nblock_boundary = \"never\"\nconditional_continuation = false"),
            config("style = \"leading-comma\""),
            config(
                "style = \"leading-comma\"\nmax_width = 40\nindent = 4\nclose_paren = \"attached\"",
            ),
            config("style = \"compact\""),
            config("style = \"classic\""),
        ] {
            fmt_with(&text, &config);
        }
    }
}

#[test]
fn files_with_syntax_errors_are_not_formatted() {
    let result = format(
        "entity e is port (a : in bit) end;",
        &FormatConfig::default(),
        Standard::Vhdl2008,
    );
    assert!(matches!(result, Err(FormatError::Syntax(_))));
}

#[test]
fn empty_and_blank_files() {
    assert_eq!(fmt(""), "");
    assert_eq!(fmt("\n\n  \n"), "");
    assert_eq!(fmt("-- only a comment"), "-- only a comment\n");
}

#[test]
fn spacing() {
    assert_eq!(
        fmt(
            "architecture a of e is begin x<=-a+b(1)*(c-d)&\"01\"when e'event and f/=g else not h;end;"
        ),
        "architecture a of e is\nbegin\n  x <= -a + b(1) * (c - d) & \"01\" when e'event and f /= g else not h;\nend;\n"
    );
    assert_eq!(
        fmt(
            "package p is constant c:t:=t'(others=>'0');alias x is << signal .tb.dut.s : bit >>;end;"
        ),
        "package p is\n  constant c : t := t'(others => '0');\n  alias x is << signal .tb.dut.s : bit >>;\nend;\n"
    );
}

/// Found by the `format` fuzz target: a malformed external path keeps the
/// spaces that separate its tokens, instead of gluing `s foo` into `sfoo`.
#[test]
fn tokens_written_apart_are_not_joined() {
    for (path, formatted) in [
        (".tb.dut.s foo", ".tb.dut.s foo"),
        (".tb.dut.s \u{ff}\u{ff}", ".tb.dut.s \u{ff}\u{ff}"),
        (".a - -b", ".a- -b"),
    ] {
        let alias = |path| format!("package p is\n  alias x is << signal {path} : bit >>;\nend;\n");
        assert_eq!(fmt(&alias(path)), alias(formatted));
    }
}

#[test]
fn keyword_and_identifier_case() {
    let text = "Entity Foo Is End Entity Foo;\n";
    assert_eq!(fmt(text), "entity Foo is\nend entity Foo;\n");
    assert_eq!(
        fmt_with(
            text,
            &config("keyword_case = \"upper\"\nidentifier_case = \"lower\"")
        ),
        "ENTITY foo IS\nEND ENTITY foo;\n"
    );
    assert_eq!(
        fmt_with(
            text,
            &config("keyword_case = \"preserve\"\nidentifier_case = \"upper\"")
        ),
        "Entity FOO Is\nEnd Entity FOO;\n"
    );
    // Extended identifiers keep their case.
    assert_eq!(
        fmt_with(
            "entity \\Foo\\ is end;",
            &config("identifier_case = \"lower\"")
        ),
        "entity \\Foo\\ is\nend;\n"
    );
}

#[test]
fn indentation() {
    let text =
        "architecture a of e is begin p: process begin if c then x := 1; end if; end process; end;";
    assert_eq!(
        fmt_with(text, &config("indent = 4")),
        "architecture a of e is\nbegin\n    p : process\n    begin\n        if c then\n            x := 1;\n        end if;\n    end process;\nend;\n"
    );
    assert_eq!(
        fmt_with(text, &config("use_tabs = true")),
        "architecture a of e is\nbegin\n\tp : process\n\tbegin\n\t\tif c then\n\t\t\tx := 1;\n\t\tend if;\n\tend process;\nend;\n"
    );
}

#[test]
fn continuation_indent() {
    let text = "architecture a of e is begin\nassert x = y\nreport \"bad\";\nend;";
    assert_eq!(
        fmt(text),
        "architecture a of e is\nbegin\n  assert x = y\n    report \"bad\";\nend;\n"
    );
    assert_eq!(
        fmt_with(text, &config("continuation_indent = 6")),
        "architecture a of e is\nbegin\n  assert x = y\n        report \"bad\";\nend;\n"
    );
}

#[test]
fn a_label_on_its_own_line_does_not_indent_the_statement() {
    assert_eq!(
        fmt("architecture a of e is begin\ngen:\nif c generate\nx <= y;\nend generate;\nend;"),
        "architecture a of e is\nbegin\n  gen :\n  if c generate\n    x <= y;\n  end generate;\nend;\n"
    );
}

#[test]
fn spacing_options() {
    let text = "architecture a of e is begin lbl : process (clk) begin end process; end;";
    assert_eq!(
        fmt_with(
            text,
            &config("space_before_label_colon = false\nspace_after_keyword = false")
        ),
        "architecture a of e is\nbegin\n  lbl: process(clk)\n  begin\n  end process;\nend;\n"
    );
}

#[test]
fn end_labels_and_keywords() {
    let text = "entity e is end; architecture a of e is begin p : process begin end process; end;";
    assert_eq!(
        fmt_with(text, &config("end_label = \"add\"\nend_keyword = \"add\"")),
        "entity e is\nend entity e;\narchitecture a of e is\nbegin\n  p : process\n  begin\n  end process p;\nend architecture a;\n"
    );
    let text = "entity e is end entity e; package body p is function f return t is begin end function f; end package body p;";
    assert_eq!(
        fmt_with(
            text,
            &config("end_label = \"remove\"\nend_keyword = \"remove\"")
        ),
        "entity e is\nend;\npackage body p is\n  function f return t is\n  begin\n  end;\nend;\n"
    );
    // Required keywords stay.
    assert_eq!(
        fmt_with(
            "package p is type r is record a : bit; end record r; end;",
            &config("end_label = \"remove\"\nend_keyword = \"remove\"")
        ),
        "package p is\n  type r is record\n    a : bit;\n  end record;\nend;\n"
    );
}

#[test]
fn blank_lines() {
    let text = "\n\npackage p is\n\n\n\n  constant a : bit := '0';\nend;\n\n\n";
    assert_eq!(
        fmt(text),
        "package p is\n\n\n  constant a : bit := '0';\nend;\n"
    );
    assert_eq!(
        fmt_with(text, &config("max_blank_lines = 0")),
        "package p is\n  constant a : bit := '0';\nend;\n"
    );
}

#[test]
fn interface_list_layout() {
    let text = "entity e is port (a : in bit; b : out bit); end;";
    assert_eq!(
        fmt(text),
        "entity e is\n  port (\n    a : in  bit;\n    b : out bit\n  );\nend;\n"
    );
    assert_eq!(
        fmt_with(text, &config("interface_list = \"auto\"")),
        "entity e is\n  port (a : in bit; b : out bit);\nend;\n"
    );
    assert_eq!(
        fmt_with(text, &config("close_paren = \"attached\"")),
        "entity e is\n  port (\n    a : in  bit;\n    b : out bit);\nend;\n"
    );
    // `preserve` keeps the breaks as written.
    let text = "entity e is port (a : in bit;\n b : out bit); end;";
    assert_eq!(
        fmt_with(text, &config("interface_list = \"preserve\"")),
        "entity e is\n  port (a : in bit;\n    b : out bit);\nend;\n"
    );
}

#[test]
fn association_list_layout() {
    let text = "architecture a of e is begin u : entity work.x port map (a => b, c => d); end;";
    assert_eq!(
        fmt(text),
        "architecture a of e is\nbegin\n  u : entity work.x\n    port map (a => b, c => d);\nend;\n"
    );
    assert_eq!(
        fmt_with(text, &config("association_list = \"expand\"")),
        "architecture a of e is\nbegin\n  u : entity work.x\n    port map (\n      a => b,\n      c => d\n    );\nend;\n"
    );
    // `auto` expands a list that is already split.
    assert_eq!(
        fmt("architecture a of e is begin u : entity work.x port map (a => b,\nccc => d); end;"),
        "architecture a of e is\nbegin\n  u : entity work.x\n    port map (\n      a   => b,\n      ccc => d\n    );\nend;\n"
    );
}

#[test]
fn long_lines_are_broken_at_lists() {
    let text = "package p is procedure log(constant level : in natural; constant message : in string); end;";
    assert_eq!(
        fmt_with(text, &config("max_width = 60")),
        "package p is\n  procedure log(\n    constant level   : in natural;\n    constant message : in string\n  );\nend;\n"
    );
    assert_eq!(
        fmt_with(text, &config("max_width = 100")),
        "package p is\n  procedure log(constant level : in natural; constant message : in string);\nend;\n"
    );
}

#[test]
fn case_alternatives() {
    let text = "architecture a of e is begin process begin case s is when A => x <= 1; when BB => x <= 22; end case; end process; end;";
    assert_eq!(
        fmt(text),
        "architecture a of e is\nbegin\n  process\n  begin\n    case s is\n      when A  => x <= 1;\n      when BB => x <= 22;\n    end case;\n  end process;\nend;\n"
    );
    assert_eq!(
        fmt_with(text, &config("case_alternative = \"break\"")),
        "architecture a of e is\nbegin\n  process\n  begin\n    case s is\n      when A =>\n        x <= 1;\n      when BB =>\n        x <= 22;\n    end case;\n  end process;\nend;\n"
    );
}

#[test]
fn line_endings_and_bom() {
    let text = "\u{feff}entity e is\r\nend;\r\n";
    assert_eq!(fmt(text), text);
    assert_eq!(
        fmt_with(text, &config("line_ending = \"lf\"")),
        "\u{feff}entity e is\nend;\n"
    );
    assert_eq!(
        fmt_with("entity e is\nend;\n", &config("line_ending = \"crlf\"")),
        "entity e is\r\nend;\r\n"
    );
}

const ALIGN_SAMPLE: &str = "\
entity e is
  port (
    clk : in std_logic; -- clock
    data_in : in std_logic_vector(7 downto 0) := (others => '0');

    q : out std_logic -- output
  );
end;
architecture a of e is
  signal s : bit;
  constant long_name : bit := '0';
begin
  s <= a;
  long_name_2 <= b;
  y <= c when d else
  e;
end;
";

#[test]
fn alignment_defaults() {
    assert_eq!(
        fmt(ALIGN_SAMPLE),
        "\
entity e is
  port (
    clk     : in std_logic; -- clock
    data_in : in std_logic_vector(7 downto 0) := (others => '0');

    q : out std_logic -- output
  );
end;
architecture a of e is
  signal s           : bit;
  constant long_name : bit := '0';
begin
  s           <= a;
  long_name_2 <= b;
  y           <= c when d else
                 e;
end;
"
    );
}

#[test]
fn alignment_can_be_turned_off() {
    let off = config(
        "[format.align]\nport_colon = false\nport_mode = false\ninterface_default = false\n\
         assignment = false\ndeclaration = false\ntrailing_comment = false\n\
         conditional_continuation = false",
    );
    assert_eq!(
        fmt_with(ALIGN_SAMPLE, &off),
        "\
entity e is
  port (
    clk : in std_logic; -- clock
    data_in : in std_logic_vector(7 downto 0) := (others => '0');

    q : out std_logic -- output
  );
end;
architecture a of e is
  signal s : bit;
  constant long_name : bit := '0';
begin
  s <= a;
  long_name_2 <= b;
  y <= c when d else
    e;
end;
"
    );
}

#[test]
fn alignment_block_boundaries() {
    let text = "entity e is port (\na : in bit;\n\n-- group two\nbbb : in bit); end;";
    assert_eq!(
        fmt(text),
        "entity e is\n  port (\n    a : in bit;\n\n    -- group two\n    bbb : in bit\n  );\nend;\n"
    );
    assert_eq!(
        fmt_with(text, &config("[format.align]\nblock_boundary = \"never\"")),
        "entity e is\n  port (\n    a   : in bit;\n\n    -- group two\n    bbb : in bit\n  );\nend;\n"
    );
    let text = "entity e is port (\na : in bit;\n-- comment\nbbb : in bit); end;";
    assert_eq!(
        fmt(text),
        "entity e is\n  port (\n    a   : in bit;\n    -- comment\n    bbb : in bit\n  );\nend;\n"
    );
    assert_eq!(
        fmt_with(
            text,
            &config("[format.align]\nblock_boundary = \"comment\"")
        ),
        "entity e is\n  port (\n    a : in bit;\n    -- comment\n    bbb : in bit\n  );\nend;\n"
    );
}

#[test]
fn generic_and_association_alignment() {
    let text = "entity e is generic (a : natural := 1; bbb : boolean := true); end;\n\
                architecture x of e is begin u : entity work.e generic map (\na => 1, bbb => false); end;";
    assert_eq!(
        fmt(text),
        "entity e is\n  generic (\n    a   : natural := 1;\n    bbb : boolean := true\n  );\nend;\n\
         architecture x of e is\nbegin\n  u : entity work.e\n    generic map (\n      a   => 1,\n      bbb => false\n    );\nend;\n"
    );
    assert_eq!(
        fmt_with(
            text,
            &config("[format.align]\ngeneric_colon = false\nassociation = false")
        ),
        "entity e is\n  generic (\n    a : natural   := 1;\n    bbb : boolean := true\n  );\nend;\n\
         architecture x of e is\nbegin\n  u : entity work.e\n    generic map (\n      a => 1,\n      bbb => false\n    );\nend;\n"
    );
}

#[test]
fn when_and_case_choice_alignment() {
    let text = "architecture a of e is begin with s select y <= a when \"0\",\nbbb when others;\n\
                process begin case s is when A => x <= 1; when BB => x <= 2; end case; end process; end;";
    assert_eq!(
        fmt(text),
        "architecture a of e is\nbegin\n  with s select y <= a   when \"0\",\n                     bbb when others;\n  process\n  begin\n    case s is\n      when A  => x <= 1;\n      when BB => x <= 2;\n    end case;\n  end process;\nend;\n"
    );
    assert_eq!(
        fmt_with(
            text,
            &config("[format.align]\nwhen_clause = false\ncase_choice = false")
        ),
        "architecture a of e is\nbegin\n  with s select y <= a when \"0\",\n                     bbb when others;\n  process\n  begin\n    case s is\n      when A => x <= 1;\n      when BB => x <= 2;\n    end case;\n  end process;\nend;\n"
    );
}

#[test]
fn verbatim_regions() {
    let text = "architecture a of e is begin\n-- vfl: off\nx<=y;\n-- vfl: on\nz<=w;\n-- VFL: skip\nkeep  <=  this;\nq<=r;\nend;";
    assert_eq!(
        fmt(text),
        "architecture a of e is\nbegin\n  -- vfl: off\nx<=y;\n-- vfl: on\n  z <= w;\n  -- VFL: skip\nkeep  <=  this;\n  q <= r;\nend;\n"
    );
    // Without `on`, the rest of the file is kept.
    assert_eq!(
        fmt("entity e is\n-- vfl: off\nport(a:in bit);end;\n"),
        "entity e is\n  -- vfl: off\nport(a:in bit);end;\n"
    );
}

const INSTANCE: &str = "architecture a of e is begin
u0 : entity work.sub generic map (W => 8, D => 2)
port map (clk => clk, -- the clock
-- the data
data_in => cnt,
q => open);
end;
";

#[test]
fn leading_separators() {
    let text = "entity e is generic (W : natural := 8; -- width
D : natural := 2); port (clk : in bit; q : out bit); end;";
    assert_eq!(
        fmt_with(text, &config("interface_separator = \"leading\"")),
        "entity e is
  generic (
    W : natural := 8 -- width
  ; D : natural := 2
  );
  port (
    clk : in  bit
  ; q   : out bit
  );
end;
"
    );
    assert_eq!(
        fmt_with(
            INSTANCE,
            &config("association_separator = \"leading\"\nassociation_list = \"expand\"")
        ),
        "architecture a of e is
begin
  u0 : entity work.sub
    generic map (
      W => 8
    , D => 2
    )
    port map (
      clk     => clk -- the clock
      -- the data
    , data_in => cnt
    , q       => open
    );
end;
"
    );
    // Wider indentation: the separator still hangs left of the elements.
    assert_eq!(
        fmt_with(
            INSTANCE,
            &config("style = \"leading-comma\"\nindent = 4\nclose_paren = \"attached\"")
        ),
        "architecture a of e is
begin
    u0 : entity work.sub
        generic map (
            W => 8
          , D => 2)
        port map (
            clk     => clk -- the clock
            -- the data
          , data_in => cnt
          , q       => open);
end;
"
    );
}

#[test]
fn trailing_separators_undo_leading_ones() {
    let leading = fmt_with(INSTANCE, &config("style = \"leading-comma\""));
    let config = config("association_list = \"expand\"");
    assert_eq!(fmt_with(&leading, &config), fmt_with(INSTANCE, &config));
}

#[test]
fn leading_separators_stay_inside_one_line_lists() {
    let config = config("association_separator = \"leading\"");
    assert_eq!(
        fmt_with("architecture a of e is begin x <= f(a, b); end;", &config),
        "architecture a of e is\nbegin\n  x <= f(a, b);\nend;\n"
    );
    // A list kept as written gets its separators moved, not hung.
    assert_eq!(
        fmt_with("architecture a of e is begin x <= f(a,\nb); end;", &config),
        "architecture a of e is\nbegin\n  x <= f(a\n    , b);\nend;\n"
    );
}

#[test]
fn separators_do_not_cross_directives() {
    let text = "architecture a of e is begin
u0 : entity work.sub port map (a => b, -- vfl: skip
c => d);
end;
";
    fmt_with(text, &config("style = \"leading-comma\""));
}

#[test]
fn style_presets() {
    assert_eq!(config("style = \"default\""), FormatConfig::default());
    let leading = config("style = \"leading-comma\"");
    assert_eq!(
        leading.association_separator,
        vfl_config::Separator::Leading
    );
    assert_eq!(leading.interface_separator, vfl_config::Separator::Leading);
    // Keys set next to the style win over it.
    let classic = config("style = \"classic\"\nindent = 3\n[format.align]\nassignment = false");
    assert_eq!(classic.indent, 3);
    assert_eq!(classic.keyword_case, vfl_config::Case::Upper);
    assert!(!classic.align.assignment);
    assert!(classic.align.port_colon);
    let compact = config("style = \"compact\"\n[format.align]\nport_colon = true");
    assert!(compact.align.port_colon);
    assert!(!compact.align.assignment);
    assert!(Config::from_toml_str("[format]\nstyle = \"nope\"").is_err());
}
