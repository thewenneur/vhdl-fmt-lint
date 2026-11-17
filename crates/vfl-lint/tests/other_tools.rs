//! The rules that other VHDL tools have (compilers, Sigasi, GHDL,
//! vhdl_ls, RTL linters), on small files: what each reports, what
//! it does not, its message and its fix.

use std::path::PathBuf;
use std::sync::Arc;

use vfl_config::Config;
use vfl_lint::{Diagnostic, FileSettings, apply_fixes, lint};
use vfl_sema::{Analysis, FileInput};
use vfl_syntax::{Standard, parse};

/// The findings of `files` (name, text) in `work`, with `config` (a
/// `vfl.toml`), as `file:line: rule: message`, and the diagnostics.
fn run(files: &[(&str, &str)], config: &str) -> (Vec<String>, Vec<Diagnostic>) {
    let config = Config::from_toml_str(config).unwrap();
    let parses: Vec<_> = files
        .iter()
        .map(|(_, text)| parse(text, Standard::Vhdl2008))
        .collect();
    let inputs = files
        .iter()
        .zip(&parses)
        .map(|((name, text), parse)| FileInput {
            path: PathBuf::from(name),
            text: Arc::from(*text),
            green: parse.green().clone(),
            libraries: vec!["work".into()],
        })
        .collect();
    let analysis = Analysis::new(inputs);
    let settings: Vec<FileSettings> = files
        .iter()
        .zip(&parses)
        .map(|((name, _), parse)| FileSettings {
            file: analysis.file_id(&PathBuf::from(name)).unwrap(),
            syntax_errors: parse.errors(),
            config: &config,
            verilog: None,
            root: None,
            library_conflict: None,
            target: None,
        })
        .collect();
    let result = lint(&analysis, &settings);
    assert!(
        result.config_errors.is_empty(),
        "{:?}",
        result.config_errors
    );
    let lines = result
        .diagnostics
        .iter()
        .map(|d| {
            let file = &analysis.files()[d.file as usize];
            let at = file.line_index.line_col(&file.text, d.range.start());
            format!(
                "{}:{}: {}: {}",
                file.path.display(),
                at.line + 1,
                d.rule,
                d.message
            )
        })
        .collect();
    (lines, result.diagnostics)
}

/// The findings of one file, `test.vhd`, with only `rules` on.
fn findings(text: &str, rules: &[&str]) -> Vec<String> {
    run(&[("test.vhd", text)], &only(rules)).0
}

/// A config with the `none` preset and `rules` at `warn`.
fn only(rules: &[&str]) -> String {
    let mut config = String::from("[lint]\npreset = \"none\"\n[lint.rules]\n");
    for rule in rules {
        config.push_str(&format!("\"{rule}\" = \"warn\"\n"));
    }
    config
}

/// `text` with the fix of its only finding of `rule` applied.
#[track_caller]
fn fixed(text: &str, rule: &str) -> String {
    let (lines, diagnostics) = run(&[("test.vhd", text)], &only(&[rule]));
    let [diagnostic] = diagnostics.as_slice() else {
        panic!("expected one finding: {lines:#?}");
    };
    apply_fixes(text, diagnostic.fix.as_ref()).0
}

/// A config with the `none` preset, `rules` at `warn` and the VHDL-93
/// standard.
fn only_93(rules: &[&str]) -> String {
    format!("[project]\nstandard = \"93\"\n{}", only(rules))
}

#[test]
fn read_only_targets() {
    let text = "entity test is
  generic (N : natural := 4);
  port (a : in bit; b : out bit);
end entity;
architecture rtl of test is
  constant C : bit := '0';
begin
  process (a)
  begin
    a <= '1';
    for i in 0 to 3 loop
      i := 2;
    end loop;
    b <= a;
  end process;
end architecture;
";
    assert_eq!(
        findings(text, &["sema/read-only-target"]),
        [
            "test.vhd:10: sema/read-only-target: `a` is an input port and cannot be assigned",
            "test.vhd:12: sema/read-only-target: `i` is a loop parameter and cannot be assigned",
        ]
    );
}

#[test]
fn assignment_kind() {
    let text = "entity test is end entity;
architecture rtl of test is
  signal s : bit;
begin
  process
    variable v : bit;
  begin
    s := '1';
    v <= '0';
    v := '1';
    s <= v;
    wait;
  end process;
end architecture;
";
    assert_eq!(
        findings(text, &["sema/assignment-kind"]),
        [
            "test.vhd:8: sema/assignment-kind: `s` is a signal: assign it with `<=`",
            "test.vhd:9: sema/assignment-kind: `v` is a variable: assign it with `:=`",
        ]
    );
    let one = "entity test is end entity;
architecture rtl of test is
  signal s : bit;
begin
  process begin s := '1'; wait; end process;
end architecture;
";
    assert!(fixed(one, "sema/assignment-kind").contains("s <= '1';"));
}

#[test]
fn misplaced_statements_and_return_values() {
    let text = "entity test is end entity;
architecture rtl of test is
  function f (x : bit) return bit is
  begin
    if x = '1' then
      return;
    end if;
    return x;
  end function;
  function g (x : bit) return bit is
  begin
  end function;
  procedure p is
  begin
    return '1';
  end procedure;
begin
  process
  begin
    exit;
    return;
    wait;
  end process;
end architecture;
";
    assert_eq!(
        findings(text, &["sema/misplaced-statement", "sema/return-value"]),
        [
            "test.vhd:6: sema/return-value: a function's `return` must give a value",
            "test.vhd:10: sema/return-value: this function has no `return` statement",
            "test.vhd:15: sema/return-value: a procedure's `return` cannot give a value",
            "test.vhd:20: sema/misplaced-statement: `exit` outside a loop",
            "test.vhd:21: sema/misplaced-statement: `return` outside a function or procedure",
        ]
    );
}

#[test]
fn component_mismatch() {
    let leaf = "entity leaf is
  generic (N : natural);
  port (a : in bit; b : out bit);
end entity;
architecture rtl of leaf is begin b <= a; end architecture;
";
    let top = "entity top is end entity;
architecture rtl of top is
  component leaf is
    port (a : in bit; b : in bit; c : out bit);
  end component;
  component other is port (a : in bit); end component;
begin
end architecture;
";
    let (lines, _) = run(
        &[("leaf.vhd", leaf), ("top.vhd", top)],
        &only(&["sema/component-mismatch"]),
    );
    assert_eq!(
        lines,
        [
            "top.vhd:3: sema/component-mismatch: component `leaf` does not match its entity: port `b` is `in` here but `out` in the entity; port `c` is not in the entity; the entity's generic `N` is missing and has no default"
        ]
    );
}

#[test]
fn processes_without_waits_and_with_waits() {
    let text = "entity test is end entity;
architecture rtl of test is
  signal s : bit;
  procedure tick is begin wait for 1 ns; end procedure;
begin
  process begin s <= not s; end process;
  process (s) begin wait for 1 ns; end process;
  process begin tick; end process;
  process begin wait; end process;
end architecture;
";
    assert_eq!(
        findings(
            text,
            &["sema/missing-wait", "sema/wait-in-sensitized-process"]
        ),
        [
            "test.vhd:6: sema/missing-wait: process without a sensitivity list or a `wait`: it loops forever at time 0",
            "test.vhd:7: sema/wait-in-sensitized-process: a process with a sensitivity list cannot contain `wait`",
        ]
    );
}

#[test]
fn hidden_declarations() {
    let text = "entity test is port (a : in bit); end entity;
architecture rtl of test is
  signal s : bit;
begin
  process (a)
    variable s : bit;
    variable a : bit;
  begin
    s := a;
  end process;
end architecture;
";
    assert_eq!(
        findings(text, &["sema/hidden-declaration"]),
        [
            "test.vhd:6: sema/hidden-declaration: variable `s` hides the signal of the same name declared at line 3",
            "test.vhd:7: sema/hidden-declaration: variable `a` hides the port of the same name declared at line 1",
        ]
    );
}

const CLOCKED: &str = "library ieee; use ieee.std_logic_1164.all;
entity test is
  port (clk, clk2, en, rst, d : in std_logic; q, q2 : out std_logic);
end entity;
";

#[test]
fn clocks_used_as_data_mixed_and_gated() {
    let text = format!(
        "{CLOCKED}architecture rtl of test is
  signal gclk, r : std_logic;
begin
  gclk <= clk and en;
  process (clk) begin
    if rising_edge(clk) then r <= d; end if;
  end process;
  process (clk) begin
    if falling_edge(clk) then q <= r and clk; end if;
  end process;
  process (gclk) begin
    if rising_edge(gclk) then q2 <= d; end if;
  end process;
end architecture;
"
    );
    assert_eq!(
        findings(
            &text,
            &[
                "sema/clock-as-data",
                "sema/mixed-clock-edges",
                "sema/gated-clock"
            ]
        ),
        [
            "test.vhd:8: sema/gated-clock: clock `gclk` is made by logic: a gated clock can glitch and skips the clock network; use a clock enable or a clock buffer",
            "test.vhd:8: sema/clock-as-data: clock `clk` is used as data: its value feeds logic instead of clocking registers",
            "test.vhd:13: sema/mixed-clock-edges: `clk` is used on its falling edge here and on its rising edge at line 10",
            "test.vhd:13: sema/clock-as-data: clock `clk` is used as data: its value feeds logic instead of clocking registers",
        ]
    );
}

#[test]
fn unconstrained_integers() {
    let text = "entity test is port (n : in natural; m : in integer range 0 to 7); end entity;
architecture rtl of test is
  signal a : integer;
  signal b : natural range 0 to 15;
begin
end architecture;
";
    assert_eq!(
        findings(text, &["sema/unconstrained-integer"]),
        [
            "test.vhd:1: sema/unconstrained-integer: port `n` is an integer without a range: synthesis makes it 32 bits; give it a range",
            "test.vhd:3: sema/unconstrained-integer: signal `a` is an integer without a range: synthesis makes it 32 bits; give it a range",
        ]
    );
}

#[test]
fn reset_sources() {
    let text = format!(
        "{CLOCKED}architecture rtl of test is
  signal arst, rst2 : std_logic;
begin
  arst <= rst or en;
  process (clk) begin
    if rising_edge(clk) then rst2 <= rst; end if;
  end process;
  process (clk, arst) begin
    if arst = '1' then q <= '0'; elsif rising_edge(clk) then q <= d; end if;
  end process;
  process (clk2, rst2) begin
    if rst2 = '1' then q2 <= '0'; elsif rising_edge(clk2) then q2 <= d; end if;
  end process;
end architecture;
"
    );
    assert_eq!(
        findings(&text, &["sema/reset-source"]),
        [
            "test.vhd:13: sema/reset-source: asynchronous reset `arst` comes from logic (line 8): a glitch resets the registers",
            "test.vhd:16: sema/reset-source: asynchronous reset `rst2` is registered on `clk` but resets registers clocked by `clk2`: its release needs a reset synchronizer in that domain",
        ]
    );
}

#[test]
fn impure_uses_in_pure_functions() {
    let text = "entity test is end entity;
architecture rtl of test is
  signal s : bit;
  impure function now_s return bit is begin return s; end function;
  function f (x : bit) return bit is
    variable v : bit;
  begin
    v := x;
    return v and s and now_s;
  end function;
begin
end architecture;
";
    assert_eq!(
        findings(text, &["sema/impure-function"]),
        [
            "test.vhd:9: sema/impure-function: pure function `f` uses the signal `s` declared outside it; declare it `impure`",
            "test.vhd:9: sema/impure-function: pure function `f` calls the impure function `now_s`",
        ]
    );
}

#[test]
fn circular_dependencies_and_order() {
    let a = "use work.b.all;\npackage a is constant x : integer := 1; end package;\n";
    let b = "use work.a.all;\npackage b is constant y : integer := 2; end package;\n";
    let (lines, _) = run(
        &[("a.vhd", a), ("b.vhd", b)],
        &only(&["sema/circular-dependency"]),
    );
    assert_eq!(
        lines,
        [
            "a.vhd:2: sema/circular-dependency: circular dependency: `a` -> `b` -> `a`",
            "b.vhd:2: sema/circular-dependency: circular dependency: `b` -> `a` -> `b`",
        ]
    );
    let text = "architecture rtl of test is begin end architecture;
entity test is end entity;
";
    assert_eq!(
        findings(text, &["sema/declared-before"]),
        [
            "test.vhd:1: sema/declared-before: `rtl` comes before its entity `test` in this file; it must be analyzed after it"
        ]
    );
}

#[test]
fn missing_bodies() {
    let text = "package p is
  function f (x : bit) return bit;
  procedure q;
  constant c : integer;
  type pt is protected
    procedure inc;
  end protected;
end package;
package body p is
  function f (x : bit) return bit is begin return x; end function;
end package body;
";
    assert_eq!(
        findings(text, &["sema/missing-body"]),
        [
            "test.vhd:3: sema/missing-body: the package body does not define `q`",
            "test.vhd:4: sema/missing-body: deferred constant `c` has no full declaration in a package body",
            "test.vhd:5: sema/missing-body: protected type `pt` has no body",
        ]
    );
}

#[test]
fn associations() {
    let text = "entity leaf is port (a : in bit; b : out bit); end entity;
architecture rtl of leaf is begin b <= a; end architecture;
entity test is end entity;
architecture rtl of test is
  signal x, y, z : bit;
begin
  u1 : entity work.leaf port map (a => x, y);
  u2 : entity work.leaf port map (x, y, z);
  u3 : entity work.leaf port map (b => y, a => x);
end architecture;
";
    assert_eq!(
        findings(text, &["sema/association", "style/association-order"]),
        [
            "test.vhd:7: sema/association: a positional association cannot follow a named one",
            "test.vhd:8: sema/association: 3 positional associations, but the unit has 2 ports",
            "test.vhd:9: style/association-order: `a` is associated after `b` but declared before it",
        ]
    );
}

#[test]
fn ambiguous_calls() {
    let text = "package p1 is function f (x : integer) return integer; end package;
package body p1 is function f (x : integer) return integer is begin return x; end function; end package body;
package p2 is function f (x : integer) return integer; end package;
package body p2 is function f (x : integer) return integer is begin return x; end function; end package body;
use work.p1.all; use work.p2.all;
entity test is end entity;
architecture rtl of test is
  constant c : integer := f(1);
begin
end architecture;
";
    assert_eq!(
        findings(text, &["sema/ambiguous-call"]),
        [
            "test.vhd:8: sema/ambiguous-call: `f` is ambiguous: `p1` and `p2` declare it with the same parameter and result types"
        ]
    );
}

#[test]
fn configurations() {
    let text = "entity leaf is end entity;
architecture rtl of leaf is begin end architecture;
entity test is end entity;
architecture rtl of test is
  component leaf is end component;
begin
  u1 : leaf;
end architecture;
configuration cfg of test is
  for rtl
    for u1, u9 : leaf use entity work.leaf(rtl); end for;
  end for;
end configuration;
configuration cfg2 of test is
  for nope
  end for;
end configuration;
";
    assert_eq!(
        findings(text, &["sema/configuration"]),
        [
            "test.vhd:11: sema/configuration: architecture `rtl` has no instance labeled `u9`",
            "test.vhd:15: sema/configuration: entity `test` has no architecture `nope`",
        ]
    );
}

#[test]
fn attribute_specifications_and_constraints() {
    let text = "library ieee; use ieee.std_logic_1164.all;
entity test is end entity;
architecture rtl of test is
  constant c : bit := '0';
  signal s : std_logic_vector(3 downto 0);
  attribute keep : boolean;
  attribute keep of c : signal is true;
  attribute keep of s : signal is true;
  type matrix is array (natural range <>, natural range <>) of bit;
  signal m : matrix(0 to 3);
  signal n : natural(0 to 3);
  signal v : std_logic_vector range 0 to 3;
begin
end architecture;
";
    assert_eq!(
        findings(text, &["sema/attribute-specification", "sema/constraint"]),
        [
            "test.vhd:7: sema/attribute-specification: `c` is a constant, not a signal",
            "test.vhd:10: sema/constraint: `matrix` has 2 indexes, but 1 index range is given",
            "test.vhd:11: sema/constraint: `natural` is a scalar type: constrain it with `range`",
            "test.vhd:12: sema/constraint: `std_logic_vector` is an array type: constrain it with an index range in parentheses",
        ]
    );
}

#[test]
fn conflicting_use_clauses() {
    let text = "package p1 is constant k : integer := 1; end package;
package p2 is constant k : integer := 2; end package;
use work.p1.all; use work.p2.all;
entity test is end entity;
architecture rtl of test is
  constant c : integer := k;
begin
end architecture;
";
    assert_eq!(
        findings(text, &["sema/conflicting-use"]),
        [
            "test.vhd:6: sema/conflicting-use: `k` is not visible: both `p1` and `p2` declare it; select it (`p1.k`)"
        ]
    );
}

#[test]
fn output_reads_before_2008() {
    let text = "entity test is port (a : in bit; q : out bit); end entity;
architecture rtl of test is
  signal s : bit;
begin
  q <= a;
  s <= q;
end architecture;
";
    assert_eq!(
        run(&[("test.vhd", text)], &only_93(&["sema/standard-feature"])).0,
        ["test.vhd:6: sema/standard-feature: `q` is an output port: reading it requires VHDL-2008"]
    );
    assert!(findings(text, &["sema/standard-feature"]).is_empty());
}

#[test]
fn mode_views() {
    let text = "package p is
  type rec is record a : bit; b : bit; c : bit; end record;
  view v of rec is
    a : in;
    b : out;
  end view;
end package;
";
    let parse = parse(text, Standard::Vhdl2019);
    assert!(parse.errors().is_empty(), "{:?}", parse.errors());
    let analysis = Analysis::new(vec![FileInput {
        path: PathBuf::from("test.vhd"),
        text: Arc::from(text),
        green: parse.green().clone(),
        libraries: vec!["work".into()],
    }]);
    let file = analysis.file_id(&PathBuf::from("test.vhd")).unwrap();
    let lines: Vec<String> = analysis
        .extra_checks(file, &|id| id == "sema/mode-view", Standard::Vhdl2019)
        .into_iter()
        .map(|d| d.message)
        .collect();
    assert!(
        lines.iter().any(|l| l.contains("gives no mode to `c`")),
        "{lines:#?}"
    );
}

#[test]
fn process_forms_loops_and_postponed() {
    let text = "entity test is port (a : in bit); end entity;
architecture rtl of test is
  signal s : bit;
begin
  p2 : process (a, a) begin s <= a; end process;
  p3 : process begin loop end loop; end process;
  p4 : process begin loop s <= a; end loop; end process;
  p5 : process begin loop s <= a; wait on a; end loop; end process;
  p6 : postponed process (a) begin s <= a; end process;
  p7 : postponed process (a) begin s <= a after 1 ns; end process;
end architecture;
";
    let rules = [
        "sema/sensitivity-list-form",
        "sema/infinite-loop",
        "sema/postponed-delta",
    ];
    assert_eq!(
        findings(text, &rules),
        [
            "test.vhd:5: sema/sensitivity-list-form: `a` is named twice in the sensitivity list",
            "test.vhd:6: sema/infinite-loop: empty loop",
            "test.vhd:7: sema/infinite-loop: this loop has no `exit`, `return` or `wait`: it never ends, and simulation hangs",
            "test.vhd:9: sema/postponed-delta: a postponed process cannot assign a signal without a delay: it would start a delta cycle",
        ]
    );
    let fixed = fixed(
        text.replace("loop end loop", "loop wait; end loop")
            .replace("loop s <= a; end loop", "loop s <= a; wait; end loop")
            .replace(
                "p6 : postponed process (a) begin s <= a;",
                "p6 : postponed process (a) begin s <= a after 1 ns;",
            )
            .as_str(),
        "sema/sensitivity-list-form",
    );
    assert!(fixed.contains("p2 : process (a) begin"), "{fixed}");
}

#[test]
fn static_conditions_and_port_bounds() {
    let text = "entity test is port (a : in bit); end entity;
architecture rtl of test is
  constant N : integer := 4;
  signal s : bit;
  signal r : integer range 0 to 7;
  signal r2 : integer range 0 to 3;
begin
  p5 : process (a) begin if N > 8 then s <= a; end if; end process;
  p6 : process (a) begin if N > 2 then s <= a; elsif N = 4 then s <= '0'; end if; end process;
  u : entity work.sub port map (x => r);
  u2 : entity work.sub port map (x => r2);
end architecture;
entity sub is port (x : in integer range 0 to 3); end entity;
";
    assert_eq!(
        findings(text, &["sema/static-condition", "sema/port-bounds"]),
        [
            "test.vhd:8: sema/static-condition: this condition is always false: its constants make it `4 > 8`",
            "test.vhd:9: sema/static-condition: this condition is always true: its constants make it `4 > 2`",
            "test.vhd:9: sema/static-condition: this condition is always true: its constants make it `4 = 4`",
            "test.vhd:10: sema/port-bounds: `r` (0 to 7) can take values outside the range of the port it is associated with (0 to 3)",
        ]
    );
}

#[test]
fn associations_by_default_and_order_and_others() {
    let text = "entity sub is generic (g : integer := 1); port (a : in bit := '0'; b : in bit; c : out bit); end entity;
architecture rtl of sub is begin c <= a; end architecture;
entity test is port (x : in bit; y : out bit); end entity;
architecture rtl of test is
  type st is (s0, s1);
  signal s : st;
  signal v : bit;
begin
  u1 : entity work.sub port map (c => y, b => x);
  u2 : entity work.sub port map (a => x, b => x, c => open);
  process (s) begin
    case s is when s0 => v <= '0'; when s1 => v <= '1'; when others => v <= '0'; end case;
  end process;
end architecture;
";
    assert_eq!(
        findings(
            text,
            &[
                "sema/default-association",
                "sema/unneeded-others",
                "style/association-order"
            ]
        ),
        [
            "test.vhd:9: sema/default-association: input port `a` is not associated and takes its default value",
            "test.vhd:9: style/association-order: `b` is associated after `c` but declared before it",
            "test.vhd:12: sema/unneeded-others: the other choices already cover every value: `when others` is never taken",
        ]
    );
}

const RTL: &str = "library ieee; use ieee.std_logic_1164.all;
entity test is port (clka, clkb, rst, a : in std_logic; d : in std_logic_vector(3 downto 0); o : out std_logic_vector(3 downto 0); p, q, z : out std_logic); end entity;
architecture rtl of test is
  type st is (idle, run, done);
  signal state : st;
  signal ra, rb : std_logic_vector(3 downto 0);
  signal k, t : std_logic;
  signal enc : std_logic_vector(1 downto 0);
  constant S0 : std_logic_vector(1 downto 0) := \"00\";
  constant S1 : std_logic_vector(1 downto 0) := \"01\";
begin
  process (clka, rst) begin
    if rst = '1' then ra <= (others => '0'); k <= '1'; state <= idle;
    elsif rising_edge(clka) then
      ra <= d;
      case state is when idle => state <= run; when run => state <= done; when done => state <= idle; end case;
    end if;
  end process;
  process (clkb) begin
    if rising_edge(clkb) then
      rb <= ra;
      case enc is when S0 => enc <= S1; when S1 => enc <= S0; when others => enc <= S0; end case;
    end if;
  end process;
  t <= 'Z' when a = '1' else k;
  o <= rb;
  p <= k;
  q <= t and a;
  z <= 'X';
end architecture;
";

#[test]
fn register_transfer_rules() {
    assert_eq!(
        findings(
            RTL,
            &[
                "sema/superfluous-reset",
                "sema/multi-bit-synchronizer",
                "sema/unsafe-fsm",
                "sema/state-type",
                "sema/internal-tristate",
                "sema/unregistered-output",
                "sema/metavalue-assignment",
            ]
        ),
        [
            "test.vhd:5: sema/unsafe-fsm: state machine `state` has no safe encoding: an upset can leave it in an illegal state it never leaves; set your synthesis tool's safe-state attribute",
            "test.vhd:8: sema/state-type: state register `enc` is a vector of named constants: an enumeration type lets tools check and encode the states",
            "test.vhd:13: sema/superfluous-reset: `k` is reset but never assigned on the clock edge: it is a constant, and needs no register or reset",
            "test.vhd:21: sema/multi-bit-synchronizer: the bits of `ra` cross from `clka` to `clkb` through one register each: they can arrive in different cycles; use a handshake, a FIFO or a Gray code",
            "test.vhd:25: sema/internal-tristate: internal tri-state on `t`: FPGAs have none inside; synthesis makes multiplexers or fails",
            "test.vhd:28: sema/unregistered-output: output `q` is driven by combinational logic, not by a register",
            "test.vhd:29: sema/metavalue-assignment: `z` gets the metavalue 'X': simulation propagates it while synthesis picks any value",
        ]
    );
}

#[test]
fn style_structure_rules() {
    let text = "library ieee; library work; use ieee.std_logic_1164.all;
entity test is port (a, b, c : in bit; n : in integer; q : out bit; m : out integer; v : out bit_vector(0 to 3)); end entity;
architecture rtl of test is
  signal s1, s2 : bit;
  signal w : bit_vector(0 to 3);
  /* outer /* inner */
  constant k : integer := 1e-3;
begin
  q <= not a = b;
  m <= n + 1 sll 2;
  process (a) begin
    if a = '1' then if b = '1' then if c = '1' then if a = '1' then if b = '1' then if c = '1' then s1 <= a; end if; end if; end if; end if; end if; end if;
    for i in 0 to 3 loop w(i) <= a; end loop;
  end process;
  -- synopsys translate_off
  -- pragma foo_bar
  -- synthesis translate_on
  v <= w;
end architecture;
";
    let rules = [
        "style/nesting-depth",
        "style/multiple-declarations",
        "style/unneeded-library",
        "style/operator-precedence",
        "style/nested-comment",
        "style/literal-form",
        "style/range-direction",
        "style/unknown-pragma",
        "style/missing-label",
    ];
    assert_eq!(
        findings(text, &rules),
        [
            "test.vhd:1: style/unneeded-library: library `work` is always visible: this clause is not needed",
            "test.vhd:2: style/range-direction: vectors in this project use `downto`",
            "test.vhd:4: style/multiple-declarations: 2 objects in one declaration: declare one per line",
            "test.vhd:5: style/range-direction: vectors in this project use `downto`",
            "test.vhd:6: style/nested-comment: `/*` inside a block comment: block comments do not nest, so the first `*/` ends both",
            "test.vhd:7: style/literal-form: an integer literal cannot have a negative exponent; write a real literal (`1.0e-3`)",
            "test.vhd:9: style/operator-precedence: `not` applies to its operand only, before `=`: write `(not a) = b` or `not (a = b)`",
            "test.vhd:10: style/operator-precedence: `n + 1` is computed before `sll`: add parentheses to show it",
            "test.vhd:12: style/nesting-depth: statements nested 7 deep (more than 5): move the inner part to a subprogram or flatten the conditions",
            "test.vhd:13: style/missing-label: `loop` statement without a label",
            "test.vhd:16: style/unknown-pragma: unknown pragma `pragma foo_bar`: tools ignore it",
        ]
    );
    let fixed = fixed(text, "style/unneeded-library");
    assert!(fixed.starts_with("library ieee; use ieee"), "{fixed}");
}

#[test]
fn prohibited_constructs_and_reserved_words() {
    let text = "entity test is port (a : in bit; q : out bit; logic : in bit); end entity;
architecture rtl of test is
  shared variable sv : integer;
begin
  q <= a'delayed(1 ns);
  -- synopsys full_case
end architecture;
";
    let config = "[lint]\npreset = \"none\"\n[lint.rules]\n\"style/reserved-word\" = \"warn\"\n\"style/prohibited\" = { level = \"warn\", keywords = [\"shared\"], attributes = [\"delayed\"], pragmas = [\"full_case\"] }\n";
    assert_eq!(
        run(&[("test.vhd", text)], config).0,
        [
            "test.vhd:1: style/reserved-word: `logic` is a SystemVerilog keyword: Verilog code cannot instantiate or connect it",
            "test.vhd:3: style/prohibited: the keyword `shared` is prohibited in this project",
            "test.vhd:5: style/prohibited: the attribute `'delayed` is prohibited in this project",
            "test.vhd:6: style/prohibited: the pragma `full_case` is prohibited in this project",
        ]
    );
}

#[test]
fn encrypted_and_secondary_unit_files() {
    let entity = "entity alu is end entity;\n";
    let arch =
        "architecture rtl of alu is begin end architecture;\n-- pragma protect begin_protected\n";
    let config = only(&["project/encrypted-file", "style/secondary-unit-file"]);
    assert_eq!(
        run(&[("alu.vhd", entity), ("impl.vhd", arch)], &config).0,
        [
            "impl.vhd:1: style/secondary-unit-file: `rtl` is not in the file of its entity `alu`, and this file's name does not say whose it is",
            "impl.vhd:2: project/encrypted-file: encrypted (IEEE 1735) code: its contents are not checked",
        ]
    );
    assert!(
        run(
            &[("alu.vhd", entity), ("alu_rtl.vhd", arch)],
            &only(&["style/secondary-unit-file"])
        )
        .0
        .is_empty()
    );
}

#[test]
fn others_position_and_deprecated_packages() {
    let text = "library ieee; use ieee.std_logic_misc.all; use ieee.std_logic_1164.all;
entity test is port (a : in bit; q : out bit); end entity;
architecture rtl of test is
  signal v : bit_vector(3 downto 0);
begin
  process (a) begin
    case a is when others => q <= '0'; when '1' => q <= '1'; end case;
  end process;
  v <= (others => '0', 0 => a);
end architecture;
";
    assert_eq!(
        findings(text, &["sema/others-position", "style/deprecated-package"]),
        [
            "test.vhd:1: style/deprecated-package: `std_logic_misc` is not an IEEE package (it is Synopsys'); VHDL-2008 has its reduction operators (`and v`)",
            "test.vhd:7: sema/others-position: `others` must be the last case alternative",
            "test.vhd:9: sema/others-position: `others` must be the last association",
        ]
    );
}
