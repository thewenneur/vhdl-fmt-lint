//! The hazard rules (`sema/integer-overflow` and the others of
//! `vfl-sema`'s `code/hazards`, `vendor/inference`, `sema/logic-depth`,
//! `project/*`, `doc/stale-reference`): what each reports and what it
//! leaves alone.

use std::path::PathBuf;
use std::sync::Arc;

use vfl_config::Config;
use vfl_lint::{FileSettings, Target, apply_fixes, lint};
use vfl_sema::{Analysis, FileInput};
use vfl_syntax::{Standard, parse};

/// The findings of `files` with `config`, as `file:line: rule: message`,
/// and the fixes of each.
fn run_with(
    files: &[(&str, &str)],
    config: &str,
    target: Option<&Target>,
) -> Vec<(String, Option<vfl_lint::Fix>)> {
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
            root: Some(std::path::Path::new("/p")),
            library_conflict: None,
            target,
        })
        .collect();
    let result = lint(&analysis, &settings);
    assert!(
        result.config_errors.is_empty(),
        "{:?}",
        result.config_errors
    );
    result
        .diagnostics
        .iter()
        .map(|d| {
            let file = &analysis.files()[d.file as usize];
            let at = file.line_index.line_col(&file.text, d.range.start());
            (
                format!(
                    "{}:{}: {}: {}",
                    file.path.display(),
                    at.line + 1,
                    d.rule,
                    d.message
                ),
                d.fix.clone(),
            )
        })
        .collect()
}

/// A config with the `none` preset and `rule` at `warn`, with `options`.
fn only(rule: &str, options: &str) -> String {
    if options.is_empty() {
        format!("[lint]\npreset = \"none\"\n[lint.rules]\n\"{rule}\" = \"warn\"\n")
    } else {
        format!(
            "[lint]\npreset = \"none\"\n[lint.rules]\n\"{rule}\" = {{ level = \"warn\", {options} }}\n"
        )
    }
}

/// The findings of `rule` in one file, `test.vhd`.
fn findings(text: &str, rule: &str) -> Vec<String> {
    run_with(&[("/p/test.vhd", text)], &only(rule, ""), None)
        .into_iter()
        .map(|(l, _)| l)
        .collect()
}

/// The lines the findings of `rule` are on.
#[track_caller]
fn lines(text: &str, rule: &str) -> Vec<u32> {
    findings(text, rule)
        .iter()
        .map(|l| l.split(':').nth(1).unwrap().parse().unwrap())
        .collect()
}

/// `text` with the fix of its only finding of `rule` applied.
#[track_caller]
fn fixed(text: &str, rule: &str) -> String {
    let found = run_with(&[("/p/test.vhd", text)], &only(rule, ""), None);
    let [(_, fix)] = found.as_slice() else {
        panic!("expected one finding: {found:#?}");
    };
    apply_fixes(text, fix.as_ref()).0
}

/// An architecture `rtl` of an entity with ports, with `decls` and
/// `body`.
fn rtl(ports: &str, decls: &str, body: &str) -> String {
    format!(
        "library ieee;\nuse ieee.std_logic_1164.all;\nuse ieee.numeric_std.all;\n\
         entity test is\n  port (\n{ports}\n  );\nend entity;\n\
         architecture rtl of test is\n{decls}\nbegin\n{body}\nend architecture;\n"
    )
}

const CLK: &str = "    clk : in std_logic;\n    rst : in std_logic";

// ---- N18 vendor/inference ------------------------------------------------------

const RAM: &str = "  type mem_t is array (0 to 1023) of std_logic_vector(15 downto 0);
  signal mem : mem_t;
  signal sr : std_logic_vector(15 downto 0);
  signal p : unsigned(31 downto 0);";

fn ram_body(reset_memory: bool) -> String {
    format!(
        "  process (clk) begin
    if rising_edge(clk) then
      if rst = '1' then
        {}
        sr <= (others => '0');
      else
        mem(to_integer(addr)) <= d;
        sr <= sr(14 downto 0) & d(0);
        p <= a * b;
      end if;
    end if;
  end process;
  q <= mem(to_integer(addr));",
        if reset_memory {
            "mem <= (others => (others => '0'));"
        } else {
            "null;"
        }
    )
}

const RAM_PORTS: &str = "    clk : in std_logic;\n    rst : in std_logic;
    addr : in unsigned(9 downto 0);
    d : in std_logic_vector(15 downto 0);
    a, b : in unsigned(31 downto 0);
    q : out std_logic_vector(15 downto 0)";

#[test]
fn inference_on_amd() {
    let text = rtl(RAM_PORTS, RAM, &ram_body(true));
    let found = run_with(
        &[("/p/test.vhd", &text)],
        &only("vendor/inference", "family = \"artix7\""),
        None,
    );
    let found: Vec<&str> = found.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(found.len(), 4, "{found:#?}");
    assert!(
        found[0].contains("`mem` (1024 x 16 bits) is reset: block RAM"),
        "{found:#?}"
    );
    assert!(
        found[1].contains("`sr` is a shift register with a reset: AMD 7 series"),
        "{found:#?}"
    );
    assert!(
        found[2].contains("32 x 32-bit multiplication is wider than one AMD 7 series DSP"),
        "{found:#?}"
    );
    assert!(
        found[3].contains("`mem` (1024 x 16 bits) is read without a clock"),
        "{found:#?}"
    );
}

#[test]
fn inference_family_from_vendor_files() {
    let text = rtl(RAM_PORTS, RAM, &ram_body(false));
    let target = Target::parse(&["set_global_assignment -name FAMILY \"Cyclone V\"\n"]);
    let found = run_with(
        &[("/p/test.vhd", &text)],
        &only("vendor/inference", ""),
        Some(&target),
    );
    let found: Vec<&str> = found.iter().map(|(l, _)| l.as_str()).collect();
    // Intel has 27 x 27 DSPs and no SRL; the read without a clock stays.
    assert_eq!(found.len(), 2, "{found:#?}");
    assert!(
        found[0].contains("Intel Cyclone V/Arria V DSP"),
        "{found:#?}"
    );
    assert!(found[1].contains("read without a clock"), "{found:#?}");
}

#[test]
fn inference_without_device_only_checks_memory_resets() {
    let text = rtl(RAM_PORTS, RAM, &ram_body(true));
    let found = findings(&text, "vendor/inference");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].contains(
        "is reset: block RAM contents cannot be reset, so it is built from 16384 flip-flops"
    ));
}

// ---- N1 sema/integer-overflow --------------------------------------------------

#[test]
fn integer_overflow() {
    let text = rtl(
        CLK,
        "  signal cnt : integer range 0 to 9;\n  signal ok : integer range 0 to 9;\n  signal n : natural;",
        "  process (clk) begin
    if rising_edge(clk) then
      cnt <= cnt + 1;
      if ok = 9 then ok <= 0; else ok <= ok + 1; end if;
      n <= n + 1;
    end if;
  end process;",
    );
    let found = findings(&text, "sema/integer-overflow");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].contains(
            "`cnt` is counted up by 1 with no test of its value around it: past 9 (0 to 9)"
        ),
        "{found:#?}"
    );
}

// ---- N2 sema/dont-care-compare -------------------------------------------------

#[test]
fn dont_care_compare() {
    let text = rtl(
        "    op : in std_logic_vector(3 downto 0);\n    y : out std_logic",
        "",
        "  y <= '1' when op = \"1--0\" else '0';
  process (op) begin
    case op is
      when \"1-11\" => null;
      when others => null;
    end case;
    case? op is
      when \"1-11\" => null;
      when others => null;
    end case?;
  end process;",
    );
    assert_eq!(lines(&text, "sema/dont-care-compare"), [13, 16]);
    let fixed = fixed(
        &rtl(
            "    op : in std_logic_vector(3 downto 0);\n    y : out std_logic",
            "",
            "  y <= '1' when op /= \"1--0\" else '0';",
        ),
        "sema/dont-care-compare",
    );
    assert!(
        fixed.contains("when not std_match(op, \"1--0\") else"),
        "{fixed}"
    );
}

// ---- N3 sema/latency-mismatch, N4 sema/handshake ---------------------------------

#[test]
fn latency_mismatch() {
    let ports = "    clk : in std_logic;
    s_data : in std_logic_vector(7 downto 0);
    s_valid : in std_logic;
    m_data : out std_logic_vector(7 downto 0);
    m_valid : out std_logic";
    let decls = "  signal d1, d2 : std_logic_vector(7 downto 0);\n  signal v1 : std_logic;";
    let body = |data: &str| {
        format!(
            "  process (clk) begin
    if rising_edge(clk) then
      v1 <= s_valid;
      d1 <= s_data;
      d2 <= d1;
      m_valid <= v1;
      m_data <= {data};
    end if;
  end process;"
        )
    };
    let found = findings(&rtl(ports, decls, &body("d2")), "sema/latency-mismatch");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].contains("`m_data` is 3 registers after the inputs but `m_valid` is 2"),
        "{found:#?}"
    );
    assert!(findings(&rtl(ports, decls, &body("d1")), "sema/latency-mismatch").is_empty());
}

#[test]
fn handshake() {
    let ports = "    clk : in std_logic;
    m_ready : in std_logic;
    full : in std_logic;
    m_valid : out std_logic";
    let decls = "  signal ok : std_logic;";
    let bad = rtl(
        ports,
        decls,
        "  ok <= m_ready and not full;\n  m_valid <= ok;",
    );
    let found = findings(&bad, "sema/handshake");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].contains("`m_valid` depends on `m_ready` through combinational logic"));
    let good = rtl(
        ports,
        decls,
        "  process (clk) begin\n    if rising_edge(clk) then\n      m_valid <= m_ready;\n    end if;\n  end process;",
    );
    assert!(findings(&good, "sema/handshake").is_empty());
}

// ---- N5 sema/power-of-two-wrap ----------------------------------------------------

#[test]
fn power_of_two_wrap() {
    let text = |assert: &str| {
        format!(
            "library ieee;\nuse ieee.std_logic_1164.all;\nuse ieee.numeric_std.all;\n\
             use ieee.math_real.all;\n\
             entity fifo is\n  generic (DEPTH : positive := 12);\n  port (clk : in std_logic);\nend entity;\n\
             architecture rtl of fifo is\n\
             function clog2(n : positive) return natural is begin return integer(ceil(log2(real(n)))); end;\n\
             signal wr_ptr : unsigned(clog2(DEPTH) - 1 downto 0);\nbegin\n{assert}\
             process (clk) begin\n  if rising_edge(clk) then\n    wr_ptr <= wr_ptr + 1;\n  end if;\nend process;\n\
             end architecture;\n"
        )
    };
    let found = findings(&text(""), "sema/power-of-two-wrap");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].contains("with `DEPTH` = 12, `wr_ptr` reaches 15 past the last entry 11"),
        "{found:#?}"
    );
    let checked = text("assert DEPTH = 2 ** clog2(DEPTH) report \"DEPTH\" severity failure;\n");
    assert!(findings(&checked, "sema/power-of-two-wrap").is_empty());
}

// ---- N6 sema/init-reset-mismatch ---------------------------------------------------

#[test]
fn init_reset_mismatch() {
    let text = rtl(
        CLK,
        "  type state_t is (IDLE, WAIT_CFG, RUN);
  signal state : state_t := IDLE;
  signal count : std_logic_vector(7 downto 0) := x\"00\";",
        "  process (clk) begin
    if rising_edge(clk) then
      if rst = '1' then
        state <= WAIT_CFG;
        count <= (others => '0');
      else
        state <= RUN;
        count <= x\"01\";
      end if;
    end if;
  end process;",
    );
    let found = findings(&text, "sema/init-reset-mismatch");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].contains("`state` starts as `IDLE` (its initial value) but resets to `WAIT_CFG`")
    );
}

// ---- N7 sema/broken-sequence --------------------------------------------------------

#[test]
fn broken_sequence() {
    let text = rtl(
        "    d : in std_logic_vector(3 downto 0);\n    q : out std_logic_vector(3 downto 0)",
        "",
        "  q(0) <= d(0);\n  q(1) <= d(1);\n  q(2) <= d(1);\n  q(3) <= d(3);
  u0 : entity work.test port map (d(0) => d(0), d(1) => d(1), d(2) => d(2), d(3) => d(2), q => open);",
    );
    let found = findings(&text, "sema/broken-sequence");
    assert_eq!(found.len(), 2, "{found:#?}");
    assert!(
        found[0].contains("`q(2) <= d(1)` breaks the pattern of its 3 neighbors"),
        "{found:#?}"
    );
    assert!(found[0].contains("`d(2)` expected"), "{found:#?}");
    assert!(found[1].contains("`d(3) => d(2)`"), "{found:#?}");
}

// ---- N8 sema/branch-clone --------------------------------------------------------------

#[test]
fn branch_clone() {
    let text = rtl(
        "    sel : in std_logic_vector(1 downto 0);\n    a, b, c : in std_logic;\n    y, z : out std_logic",
        "",
        "  process (all) begin
    if sel = \"00\" then y <= a;
    elsif sel = \"01\" then y <= b;
    elsif sel = \"00\" then y <= c;
    end if;
    if a = '1' then z <= b; else z <= b; end if;
    case sel is
      when \"00\" => y <= a;
      when \"01\" => y <= a;
      when others => y <= c;
    end case;
  end process;",
    );
    let found = findings(&text, "sema/branch-clone");
    assert_eq!(found.len(), 3, "{found:#?}");
    assert!(found[0].contains("same as the one on line"), "{found:#?}");
    assert!(
        found[1].contains("every branch of this `if` does the same thing"),
        "{found:#?}"
    );
    assert!(found[2].contains("merge them"), "{found:#?}");
}

// ---- N9, N10, N11, N17: numeric_std ------------------------------------------------------

#[test]
fn numeric_std_traps() {
    let text = rtl(
        "    x : in signed(7 downto 0);\n    u : in unsigned(39 downto 0);\n    w : in unsigned(7 downto 0);\n    y : out signed(3 downto 0);\n    n : out integer",
        "  signal v, v2 : unsigned(7 downto 0);\n  signal s : signed(7 downto 0);\n  signal k : unsigned(7 downto 0);",
        "  y <= resize(x, 4);
  s <= x srl 2;
  n <= to_integer(u);
  v <= to_unsigned(300, 8);
  v2 <= unsigned(w);
  k <= to_unsigned(to_integer(w), 8);",
    );
    let first = |rule| findings(&text, rule);
    let resize = first("sema/signed-resize");
    assert_eq!(resize.len(), 1, "{resize:#?}");
    assert!(resize[0].contains("from 8 to 4 bits keeps the sign bit and the 3 low bits"));
    assert_eq!(lines(&text, "sema/signed-logical-shift"), [19]);
    let overflow = first("sema/conversion-overflow");
    assert_eq!(overflow.len(), 2, "{overflow:#?}");
    assert!(overflow[0].contains("`to_integer` of a 40-bit unsigned value"));
    assert!(overflow[1].contains("300 does not fit in 8 bits (0 to 255)"));
    let redundant = first("sema/redundant-conversion");
    assert_eq!(redundant.len(), 2, "{redundant:#?}");
    assert!(
        redundant[0].contains("`w` is already of type `unsigned`"),
        "{redundant:#?}"
    );
    assert!(
        redundant[1].contains("`resize(w, 8)` does the same"),
        "{redundant:#?}"
    );
    let shift = rtl(
        "    x : in signed(7 downto 0);\n    s : out signed(7 downto 0)",
        "",
        "  s <= x srl 2;",
    );
    assert!(fixed(&shift, "sema/signed-logical-shift").contains("s <= shift_right(x, 2);"));
    let conversion = rtl(
        "    w : in unsigned(7 downto 0);\n    v : out unsigned(7 downto 0)",
        "",
        "  v <= unsigned(w);",
    );
    assert!(fixed(&conversion, "sema/redundant-conversion").contains("v <= w;"));
}

// ---- N12 sema/unused-bits ---------------------------------------------------------------

#[test]
fn unused_bits() {
    let text = rtl(
        "    a, b : in unsigned(15 downto 0);\n    r : out unsigned(15 downto 0)",
        "  signal sum : unsigned(16 downto 0);\n  signal half : unsigned(7 downto 0);\n  signal whole : unsigned(7 downto 0);",
        "  sum <= resize(a, 17) + b;\n  r <= sum(15 downto 0) + half + whole;\n  half(3 downto 0) <= a(3 downto 0);\n  whole <= a(7 downto 0);",
    );
    let found = findings(&text, "sema/unused-bits");
    assert_eq!(found.len(), 2, "{found:#?}");
    assert!(
        found[0].contains("bit 16 of `sum` is never read"),
        "{found:#?}"
    );
    assert!(
        found[1].contains("bits 7 downto 4 of `half` are never assigned"),
        "{found:#?}"
    );
}

// ---- N13 sema/process-restarts, N14 sema/unbounded-wait -----------------------------------

#[test]
fn testbench_processes() {
    let text = "library ieee;\nuse ieee.std_logic_1164.all;\n\
        entity tb is\nend entity;\n\
        architecture sim of tb is\n  signal clk, rst, done : std_logic := '0';\nbegin\n\
        clk <= not clk after 5 ns;\n\
        gen : process begin\n  clk <= '1'; wait for 5 ns; clk <= '0'; wait for 5 ns;\nend process;\n\
        stim : process begin\n  rst <= '1';\n  wait for 100 ns;\n  rst <= '0';\n  wait until done = '1';\nend process;\n\
        ok : process begin\n  wait for 10 ns;\n  wait until done = '1' for 1 ms;\n  std.env.stop;\nend process;\n\
        end architecture;\n";
    assert_eq!(lines(text, "sema/process-restarts"), [12]);
    assert_eq!(lines(text, "sema/unbounded-wait"), [16]);
}

// ---- N15 project/constant-port ------------------------------------------------------------

#[test]
fn constant_port() {
    let filt = "library ieee;\nuse ieee.std_logic_1164.all;\n\
        entity filt is\n  port (bypass : in std_logic; d : in std_logic; q : out std_logic);\nend entity;\n\
        architecture rtl of filt is\nbegin\n  q <= d when bypass = '1' else not d;\nend architecture;\n";
    let top = |second: &str| {
        format!(
            "library ieee;\nuse ieee.std_logic_1164.all;\n\
             entity top is\n  port (a, b : in std_logic; x, y : out std_logic);\nend entity;\n\
             architecture rtl of top is\nbegin\n\
             u0 : entity work.filt port map (bypass => '0', d => a, q => x);\n\
             u1 : entity work.filt port map (bypass => {second}, d => b, q => y);\n\
             end architecture;\n"
        )
    };
    let config = only("project/constant-port", "");
    let run = |second: &str| {
        run_with(
            &[("/p/filt.vhd", filt), ("/p/top.vhd", &top(second))],
            &config,
            None,
        )
        .into_iter()
        .map(|(l, _)| l)
        .collect::<Vec<_>>()
    };
    let found = run("'0'");
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].starts_with(
            "/p/filt.vhd:4: project/constant-port: every instance (2) ties `bypass` to `'0'`"
        ),
        "{found:#?}"
    );
    assert!(run("a").is_empty());
}

// ---- N16 project/layering --------------------------------------------------------------------

#[test]
fn layering() {
    let tb =
        "entity helper is\nend entity;\narchitecture sim of helper is\nbegin\nend architecture;\n";
    let rtl_file = "library ieee;\nuse ieee.std_logic_1164.all;\nuse std.textio.all;\n\
        entity top is\nend entity;\narchitecture rtl of top is\nbegin\n\
        u0 : entity work.helper;\nend architecture;\n";
    let config = "[lint]\npreset = \"none\"\n[lint.rules]\n\
        \"project/layering\" = { level = \"error\", deny = { \"rtl/**\" = [\"std.textio\", \"tb/**\"] } }\n";
    let found: Vec<String> = run_with(
        &[("/p/rtl/top.vhd", rtl_file), ("/p/tb/helper.vhd", tb)],
        config,
        None,
    )
    .into_iter()
    .map(|(l, _)| l)
    .collect();
    assert_eq!(found.len(), 2, "{found:#?}");
    assert!(
        found[0].contains(":3: project/layering: files under `rtl/**` must not use `std.textio`"),
        "{found:#?}"
    );
    assert!(
        found[1].contains(
            ":8: project/layering: files under `rtl/**` must not instantiate `helper` (`tb/**`)"
        ),
        "{found:#?}"
    );
}

// ---- N19 sema/logic-depth --------------------------------------------------------------------

#[test]
fn logic_depth() {
    let text = rtl(
        "    clk, slow : in std_logic;\n    a, b, c, d : in unsigned(31 downto 0);\n    q, r : out unsigned(31 downto 0)",
        "  signal s1, s2 : unsigned(31 downto 0);",
        "  s1 <= a + b;\n  s2 <= s1 + c;
  process (clk) begin
    if rising_edge(clk) then
      q <= s2 + d;
      r <= a;
    end if;
  end process;",
    );
    let found = run_with(
        &[("/p/test.vhd", &text)],
        &only(
            "sema/logic-depth",
            "family = \"artix7\", clocks = { clk = 400 }",
        ),
        None,
    );
    let found: Vec<&str> = found.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].contains("about 9 levels of logic before register `q` (through `s2`, `s1`): `clk` at 400 MHz allows about 5 (2.5 ns / 0.5 ns a level on AMD 7 series)"),
        "{found:#?}"
    );
    // A slower clock, a multicycle path, or no frequency at all: nothing.
    for options in [
        "family = \"artix7\", clocks = { clk = 100 }",
        "family = \"artix7\"",
    ] {
        assert!(
            run_with(
                &[("/p/test.vhd", &text)],
                &only("sema/logic-depth", options),
                None
            )
            .is_empty(),
            "{options}"
        );
    }
    let target = Target::parse(&[
        "create_clock -period 2.5 [get_ports clk]\nset_multicycle_path 2 -setup -to [get_cells q_reg*]\n",
        "set_property PART xc7a35tcpg236-1 [current_project]\n",
    ]);
    assert!(
        run_with(
            &[("/p/test.vhd", &text)],
            &only("sema/logic-depth", ""),
            Some(&target)
        )
        .is_empty()
    );
    let target = Target::parse(&["create_clock -period 2.5 [get_ports clk]\n"]);
    assert_eq!(
        run_with(
            &[("/p/test.vhd", &text)],
            &only("sema/logic-depth", ""),
            Some(&target)
        )
        .len(),
        1
    );
}

// ---- N20 sema/swapped-association ---------------------------------------------------------------

#[test]
fn swapped_association() {
    let sub = "entity sub is\n  port (din_a, din_b : in bit);\nend entity;\narchitecture rtl of sub is\nbegin\nend architecture;\n";
    let top = "entity top is\n  port (s_din_a, s_din_b : in bit);\nend entity;\narchitecture rtl of top is\nbegin\n\
        u0 : entity work.sub port map (din_a => s_din_b, din_b => s_din_a);\n\
        u1 : entity work.sub port map (din_a => s_din_a, din_b => s_din_b);\nend architecture;\n";
    let found: Vec<String> = run_with(
        &[("/p/sub.vhd", sub), ("/p/test.vhd", top)],
        &only("sema/swapped-association", ""),
        None,
    )
    .into_iter()
    .map(|(l, _)| l)
    .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].contains("test.vhd:6:"), "{found:#?}");
}

// ---- N21 doc/stale-reference ----------------------------------------------------------------------

#[test]
fn stale_reference() {
    let text = "-- File: uart_rx.vhd\n-- Entity: uart_rx\n\
        entity test is\nend entity;\narchitecture sim of test is\nbegin\n\
        -- @verifies UART-1, UART-9\nend architecture;\n\
        -- @req UART-1 The line idles high.\n";
    let found = findings(text, "doc/stale-reference");
    assert_eq!(found.len(), 3, "{found:#?}");
    assert!(found[0].contains(":1: doc/stale-reference: the header names file `uart_rx.vhd`"));
    assert!(found[1].contains(":2: doc/stale-reference: the header names unit `uart_rx`"));
    assert!(found[2].contains(
        ":7: doc/stale-reference: no `@req` comment of the project defines requirement `UART-9`"
    ));
    assert!(
        findings(
            "-- File: test.vhd\nentity test is\nend entity;\n",
            "doc/stale-reference"
        )
        .is_empty()
    );
}

// ---- N22 sema/expensive-operator ------------------------------------------------------------------

#[test]
fn expensive_operator() {
    let text = rtl(
        "    a, b : in unsigned(15 downto 0);\n    q, r, s : out unsigned(15 downto 0)",
        "  constant N : natural := 16 / 4;",
        "  q <= a / b;\n  r <= a / 4;\n  s <= a mod 10;",
    );
    assert_eq!(lines(&text, "sema/expensive-operator"), [13, 15]);
}
