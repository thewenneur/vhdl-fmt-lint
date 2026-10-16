//! Formatter tests: snapshots of whole files, idempotency over every test
//! file in the workspace, and one test per configuration option.

use std::path::Path;

use vfl_config::{AlignConfig, AlignScope, Config, FormatConfig};
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
    fmt_with(text, &legacy())
}

/// The defaults before block alignment (0.4): most tests show their own
/// option against them. `alignment_defaults` shows the current defaults.
fn legacy() -> FormatConfig {
    FormatConfig {
        indent: 2,
        align: AlignConfig {
            scope: AlignScope::Contiguous,
            semicolon: false,
            ..AlignConfig::default()
        },
        ..FormatConfig::default()
    }
}

/// A format config from the body of a `[format]` table, on top of
/// [`legacy`] unless it picks a style.
fn config(toml: &str) -> FormatConfig {
    let mut config = Config::from_toml_str(&format!("[format]\n{toml}"))
        .unwrap()
        .format;
    if !toml.contains("style") {
        let sets = |key: &str| toml.lines().any(|l| l.trim_start().starts_with(key));
        let legacy = legacy();
        if !sets("indent") {
            config.indent = legacy.indent;
        }
        if !sets("scope") {
            config.align.scope = legacy.align.scope;
        }
        if !sets("semicolon") {
            config.align.semicolon = legacy.align.semicolon;
        }
    }
    config
}

#[test]
fn snapshots() {
    insta::glob!("data/*.vhd", |path| {
        let text = std::fs::read_to_string(path).unwrap();
        insta::assert_snapshot!(fmt_with(&text, &FormatConfig::default()));
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
            legacy(),
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
            config("max_width = 30\noperator_position = \"leading\"\ncontinuation_indent = 2"),
            config(
                "max_width = 40\noperator_position = \"trailing\"\n\
                 [format.declaration_case]\nconstant = \"upper\"\nsignal = \"lower\"\n\
                 enum_literal = \"upper\"\nlabel = \"upper\"\n\
                 [format.blank_lines]\nafter_begin = 0\nbefore_end = 0\nbefore_process = 1\n\
                 between_declaration_groups = { min = 1 }",
            ),
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
        "package p is\n\n  constant a : bit := '0';\nend;\n"
    );
    assert_eq!(
        fmt_with(text, &config("max_blank_lines = 2")),
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
        "architecture a of e is\nbegin\n  u : entity work.x\n    port map (\n      a => b,\n      c => d\n    );\nend;\n"
    );
    assert_eq!(
        fmt_with(text, &config("association_list = \"auto\"")),
        "architecture a of e is\nbegin\n  u : entity work.x\n    port map (a => b, c => d);\nend;\n"
    );
    // `auto` expands a list that is already split.
    assert_eq!(
        fmt_with(
            "architecture a of e is begin u : entity work.x port map (a => b,\nccc => d); end;",
            &config("association_list = \"auto\"")
        ),
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
    let defaults = FormatConfig::default();
    assert_eq!(
        fmt_with(ALIGN_SAMPLE, &defaults),
        "\
entity e is
    port (
        clk     : in  std_logic                                      ; -- clock
        data_in : in  std_logic_vector(7 downto 0) := (others => '0');

        q       : out std_logic                                        -- output
    );
end;
architecture a of e is
    signal s           : bit       ;
    constant long_name : bit := '0';
begin
    s           <= a;
    long_name_2 <= b;
    y           <= c when d else
                   e;
end;
"
    );
    // Lines in between, at any depth, do not end the alignment.
    let text = "architecture a of e is begin
p : process (clk) begin
if rising_edge(clk) then
if rst = '1' then count <= 0;
else q <= d; -- data
count <= count + 1;
end if;
case s is when IDLE => s <= RUN; when RUNNING =>
s <= IDLE;
when others => null;
end case;
end if;
end process;
u0 : entity work.sub port map (
clk => clk,

data_in => q
);
end;";
    assert_eq!(
        fmt_with(text, &defaults),
        "\
architecture a of e is
begin
    p : process (clk)
    begin
        if rising_edge(clk) then
            if rst = '1' then
                count <= 0              ;
            else
                q     <= d              ; -- data
                count <= count + 1      ;
            end if;
            case s is
                when IDLE    => s <= RUN;
                when RUNNING =>
                    s <= IDLE           ;
                when others  => null    ;
            end case;
        end if;
    end process;
    u0 : entity work.sub
        port map (
            clk     => clk,

            data_in => q
        );
end;
"
    );
    // The earlier defaults stay a config away.
    let contiguous =
        config("indent = 2\n[format.align]\nscope = \"contiguous\"\nsemicolon = false");
    assert_eq!(fmt_with(text, &contiguous), fmt_with(text, &legacy()));
    assert_eq!(fmt_with(ALIGN_SAMPLE, &contiguous), fmt(ALIGN_SAMPLE));
}

#[test]
fn semicolons_can_stay_unaligned() {
    let text = "architecture a of e is begin\nx <= a;\nlong_name <= b + c;\nend;";
    assert_eq!(
        fmt_with(
            text,
            &config("indent = 4\n[format.align]\nscope = \"block\"")
        ),
        "architecture a of e is\nbegin\n    x         <= a;\n    long_name <= b + c;\nend;\n"
    );
    assert_eq!(
        fmt_with(text, &FormatConfig::default()),
        "architecture a of e is\nbegin\n    x         <= a    ;\n    long_name <= b + c;\nend;\n"
    );
}

#[test]
fn contiguous_alignment() {
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
    // The arrows of both maps line up, in the same block.
    assert_eq!(
        fmt_with(
            INSTANCE,
            &config("style = \"leading-comma\"\nindent = 4\nclose_paren = \"attached\"")
        ),
        "architecture a of e is
begin
    u0 : entity work.sub
        generic map (
            W       => 8
          , D       => 2)
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
fn skip_keeps_the_comment_ending_the_statement() {
    let text = "architecture a of e is begin\n-- vfl: skip\nkeep  <=  this;   -- as written\nq<=r; -- next\nend;";
    assert_eq!(
        fmt(text),
        "architecture a of e is\nbegin\n  -- vfl: skip\nkeep  <=  this;   -- as written\n  q <= r; -- next\nend;\n"
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

const LONG_EXPRESSIONS: &str = "architecture a of e is begin\nprocess begin\n\
    if first_condition = 1 and second_condition = 2 and third_condition = 3 then\nx <= y;\n\
    end if;\nend process;\nz <= first_value when select_a = 1 else second_value;\n\
    u : entity work.x port map (a => first_input and second_input and third_input, b => c);\nend;\n";

#[test]
fn long_expressions_break_at_their_loosest_operators() {
    let broken = fmt_with(LONG_EXPRESSIONS, &config("max_width = 50"));
    assert_eq!(
        broken,
        "architecture a of e is\nbegin\n  process\n  begin\n    if first_condition = 1 and\n      \
         second_condition = 2 and\n      third_condition = 3 then\n      x <= y;\n    end if;\n  \
         end process;\n  z <= first_value when select_a = 1 else\n       second_value;\n  \
         u : entity work.x\n    port map (\n      a => first_input and\n        second_input and\n        \
         third_input,\n      b => c\n    );\nend;\n"
    );
    // The breaks written stay; nothing is joined.
    assert_eq!(fmt_with(&broken, &config("max_width = 200")), broken);
    // With the option off, only lists break.
    let off = fmt_with(
        LONG_EXPRESSIONS,
        &config("max_width = 50\nbreak_expressions = false"),
    );
    assert!(
        off.contains(
            "    if first_condition = 1 and second_condition = 2 and third_condition = 3 then\n"
        ),
        "{off}"
    );
    assert!(
        off.contains("  z <= first_value when select_a = 1 else second_value;\n"),
        "{off}"
    );
}

#[test]
fn operators_lead_or_trail_as_configured() {
    let leading = fmt_with(
        LONG_EXPRESSIONS,
        &config("max_width = 50\noperator_position = \"leading\"\ncontinuation_indent = 4"),
    );
    assert_eq!(
        leading,
        "architecture a of e is\nbegin\n  process\n  begin\n    if first_condition = 1\n        \
         and second_condition = 2\n        and third_condition = 3 then\n      x <= y;\n    end if;\n  \
         end process;\n  z <= first_value when select_a = 1 else\n       second_value;\n  \
         u : entity work.x\n    port map (\n      a => first_input\n          and second_input\n          \
         and third_input,\n      b => c\n    );\nend;\n"
    );
    // "preserve" keeps the breaks written; "trailing" moves them back.
    let wide = "max_width = 200\ncontinuation_indent = 4";
    assert_eq!(fmt_with(&leading, &config(wide)), leading);
    assert_eq!(
        fmt_with(
            &leading,
            &config(&format!("{wide}\noperator_position = \"trailing\""))
        ),
        fmt_with(
            LONG_EXPRESSIONS,
            &config("max_width = 50\ncontinuation_indent = 4")
        )
    );
    // A comment after the operator keeps the break where it is.
    let text = "architecture a of e is begin x <= a and -- why\n b; end;";
    assert_eq!(
        fmt_with(text, &config("operator_position = \"leading\"")),
        "architecture a of e is\nbegin\n  x <= a and -- why\n    b;\nend;\n"
    );
}

#[test]
fn expressions_break_outside_parentheses_first() {
    let text = "architecture a of e is begin\n\
        sum <= resize(operand_a, 16) + resize(operand_b, 16) - resize(carry_in, 16);\nend;";
    assert_eq!(
        fmt_with(text, &config("max_width = 50")),
        "architecture a of e is\nbegin\n  sum <= resize(operand_a, 16) +\n    resize(operand_b, 16) -\n    \
         resize(carry_in, 16);\nend;\n"
    );
    // An index is broken only when nothing else can be.
    let text = "architecture a of e is begin\n\
        q <= v(first_index_term + second_index_term) when s else r;\nend;";
    assert_eq!(
        fmt_with(text, &config("max_width = 40")),
        "architecture a of e is\nbegin\n  q <= v(first_index_term +\n      second_index_term) when s else\n       r;\nend;\n"
    );
}

#[test]
fn wide_characters_count_twice() {
    // 20 characters, 40 columns.
    let text = "architecture a of e is begin\n\
        v <= \"日本語の文字列はとても長いです日本語の\" & \"abc\";\nend;";
    assert_eq!(
        fmt_with(text, &config("max_width = 50")),
        "architecture a of e is\nbegin\n  v <= \"日本語の文字列はとても長いです日本語の\" &\n    \"abc\";\nend;\n"
    );
}

#[test]
fn declaration_case() {
    let text = "entity Counter is generic (width : natural := 8); \
        port (Clk : in bit; Q : out bit_vector(Width - 1 downto 0)); end;\n\
        architecture RTL of counter is\n\
        type State_T is (idle, Run);\n\
        constant max_count : natural := 2 ** WIDTH - 1;\n\
        signal State : state_t;\n\
        signal clk_div : bit;\n\
        begin\n\
        P_Count : process (CLK) begin if state = IDLE then state <= run; end if; end process;\n\
        q <= (others => clk_div);\n\
        end;\n";
    let toml = "end_label = \"add\"\n[format.declaration_case]\nentity = \"lower\"\n\
        architecture = \"lower\"\ngeneric = \"upper\"\nconstant = \"upper\"\nport = \"lower\"\n\
        signal = \"lower\"\ntype = \"lower\"\nenum_literal = \"upper\"\nlabel = \"lower\"";
    assert_eq!(
        fmt_with(text, &config(toml)),
        "entity counter is\n  generic (\n    WIDTH : natural := 8\n  );\n  port (\n    \
         clk : in  bit;\n    q   : out bit_vector(WIDTH - 1 downto 0)\n  );\nend counter;\n\
         architecture rtl of counter is\n  type state_t is (IDLE, RUN);\n  \
         constant MAX_COUNT : natural := 2 ** WIDTH - 1;\n  signal state       : state_t;\n  \
         signal clk_div     : bit;\nbegin\n  p_count : process (clk)\n  begin\n    \
         if state = IDLE then\n      state <= RUN;\n    end if;\n  end process p_count;\n  \
         q <= (others => clk_div);\nend rtl;\n"
    );
    // A name declared as kinds with different cases keeps its own.
    let text = "package p is constant Width : natural := 8; signal Width : bit; end;";
    assert_eq!(
        fmt_with(
            text,
            &config("[format.declaration_case]\nconstant = \"upper\"\nsignal = \"lower\"")
        ),
        "package p is\n  constant Width : natural := 8;\n  signal Width   : bit;\nend;\n"
    );
}

#[test]
fn blank_line_rules() {
    let text = "architecture a of e is\n  constant c : bit := '0';\n  -- the signal\n  \
        signal s : bit;\n\nbegin\n\n  p1 : process begin\n\n    null;\n\n  end process;\n  \
        -- the second\n  p2 : process begin null; end process;\n\n\nend;\n";
    let toml = "[format.blank_lines]\nbefore_begin = 0\nafter_begin = { max = 0 }\n\
        before_end = 0\nbefore_process = { min = 1 }\nbetween_declaration_groups = 1";
    assert_eq!(
        fmt_with(text, &config(toml)),
        "architecture a of e is\n  constant c : bit := '0';\n\n  -- the signal\n  \
         signal s : bit;\nbegin\n  p1 : process\n  begin\n    null;\n  end process;\n\n  \
         -- the second\n  p2 : process\n  begin\n    null;\n  end process;\nend;\n"
    );
    // Unset, the blank lines stay as written.
    assert_eq!(fmt(text), fmt_with(text, &config("[format.blank_lines]")));
    assert!(fmt(text).contains("begin\n\n  p1"));
}
