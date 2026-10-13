use super::*;
use vfl_syntax::parse;

const COUNTER: &str = "\
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

entity counter is
  generic (
    WIDTH : natural := 8;  -- bits
    STEP  : positive
  );
  port (
    clk, rst_n : in  std_logic;
    en         : in  std_logic := '1';
    q          : out unsigned(WIDTH - 1 downto 0)
  );
end entity counter;

architecture rtl of counter is
begin
  process (clk, rst_n)
  begin
    if rst_n = '0' then
      q <= (others => '0');
    elsif rising_edge(clk) then
      if en = '1' then
        q <= q + STEP;
      end if;
    end if;
  end process;
end architecture rtl;
";

fn entity(text: &str) -> Entity {
    let root = parse(text, Standard::Vhdl2008).syntax();
    let unit = entity_units(&root).next().unwrap();
    let mut entity = Entity::from_unit(&unit, "work").unwrap();
    for unit in root.children() {
        entity.learn_from(&unit);
    }
    entity
}

#[test]
fn reads_generics_and_ports() {
    let e = entity(COUNTER);
    assert_eq!(e.name, "counter");
    let generics: Vec<_> = e
        .generics
        .iter()
        .map(|g| (&*g.name, g.default.as_deref()))
        .collect();
    assert_eq!(generics, [("WIDTH", Some("8")), ("STEP", None)]);
    let ports: Vec<_> = e
        .ports
        .iter()
        .map(|p| (&*p.name, p.mode, &*p.subtype, p.default.as_deref()))
        .collect();
    assert_eq!(
        ports,
        [
            ("clk", Mode::In, "std_logic", None),
            ("rst_n", Mode::In, "std_logic", None),
            ("en", Mode::In, "std_logic", Some("'1'")),
            ("q", Mode::Out, "unsigned(WIDTH - 1 downto 0)", None),
        ]
    );
    assert_eq!(
        e.context,
        [
            "library ieee;",
            "use ieee.std_logic_1164.all;",
            "use ieee.numeric_std.all;"
        ]
    );
}

#[test]
fn instance_follows_the_format_config() {
    let e = entity(COUNTER);
    let config = Config::default();
    assert_eq!(
        instance(&e, &config).unwrap(),
        "\
u_counter : entity work.counter
  generic map (
    WIDTH => WIDTH,
    STEP  => STEP
  )
  port map (
    clk   => clk,
    rst_n => rst_n,
    en    => en,
    q     => q
  );
"
    );
    let mut config = Config::default();
    config.format.keyword_case = vfl_config::Case::Upper;
    config.format.indent = 4;
    config.generate.instance_label = "i_{entity}".into();
    let out = component_instance(&e, &config).unwrap();
    assert!(
        out.starts_with("i_counter : counter\n    GENERIC MAP (\n        WIDTH => WIDTH,"),
        "{out}"
    );
}

#[test]
fn component_matches_the_entity() {
    let e = entity(COUNTER);
    assert_eq!(
        component(&e, &Config::default()).unwrap(),
        "\
component counter is
  generic (
    WIDTH : natural := 8;
    STEP  : positive
  );
  port (
    clk   : in  std_logic;
    rst_n : in  std_logic;
    en    : in  std_logic := '1';
    q     : out unsigned(WIDTH - 1 downto 0)
  );
end component counter;
"
    );
}

#[test]
fn testbench_drives_clock_and_reset() {
    let e = entity(COUNTER);
    let out = testbench(&e, &Config::default()).unwrap();
    insta_like(
        &out,
        "\
library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

entity counter_tb is
end entity counter_tb;

architecture sim of counter_tb is
  constant WIDTH : natural  := 8;
  constant STEP  : positive := 1; -- TODO: choose a value

  constant CLK_PERIOD : time := 10 ns;

  signal clk   : std_logic := '0';
  signal rst_n : std_logic := '0';
  signal en    : std_logic := '1';
  signal q     : unsigned(WIDTH - 1 downto 0);

  signal done : boolean := false;
begin
  dut : entity work.counter
    generic map (
      WIDTH => WIDTH,
      STEP  => STEP
    )
    port map (
      clk   => clk,
      rst_n => rst_n,
      en    => en,
      q     => q
    );

  clk <= not clk after CLK_PERIOD / 2 when not done else '0';

  stimulus : process
  begin
    rst_n <= '0';
    for i in 1 to 5 loop
      wait until rising_edge(clk);
    end loop;
    rst_n <= '1';

    -- TODO: drive the inputs and check the outputs.
    for i in 1 to 100 loop
      wait until rising_edge(clk);
    end loop;

    report \"counter_tb: done\";
    done <= true;
    std.env.finish;
    wait;
  end process stimulus;
end architecture sim;
",
    );
    // The testbench is valid VHDL.
    assert!(parse(&out, Standard::Vhdl2008).errors().is_empty());
}

fn insta_like(actual: &str, expected: &str) {
    if actual != expected {
        panic!("expected:\n{expected}\nactual:\n{actual}");
    }
}

#[test]
fn names_alone_find_clocks_and_resets() {
    let e = entity(
        "entity e is port (sys_clk : in bit; clk_en : in bit; areset : in bit; rstn : in bit; d : in bit_vector); end;",
    );
    let config = GenerateConfig::default();
    let clocks: Vec<_> = e
        .clocks(&config)
        .unwrap()
        .iter()
        .map(|p| p.name.clone())
        .collect();
    assert_eq!(clocks, ["sys_clk"]);
    let resets = e.resets(&config).unwrap();
    assert_eq!(
        resets,
        [
            Reset {
                name: "areset".into(),
                active_low: false
            },
            Reset {
                name: "rstn".into(),
                active_low: true
            },
        ]
    );
    let out = testbench(&e, &Config::default()).unwrap();
    assert!(
        out.contains(
            "signal d       : bit_vector(7 downto 0) := (others => '0'); -- TODO: the width"
        ),
        "{out}"
    );
    assert!(parse(&out, Standard::Vhdl2008).errors().is_empty(), "{out}");
}

#[test]
fn vhdl93_testbench_stops_by_starving_events() {
    let e = entity("entity e is port (clk : in bit); end;");
    let mut config = Config::default();
    config.project.standard = vfl_config::StandardSetting::Vhdl1993;
    let out = testbench(&e, &config).unwrap();
    assert!(!out.contains("std.env"), "{out}");
    assert!(out.contains("done <= true;"));
    assert!(parse(&out, Standard::Vhdl1993).errors().is_empty(), "{out}");
}

#[test]
fn entity_without_ports_or_clock() {
    let e = entity("entity top is generic (type T; function f return T is <>); end;");
    let out = testbench(&e, &Config::default()).unwrap();
    assert!(parse(&out, Standard::Vhdl2008).errors().is_empty(), "{out}");
    assert!(out.contains("subtype T is integer;"), "{out}");
    let inst = instance(&e, &Config::default()).unwrap();
    assert_eq!(
        inst,
        "u_top : entity work.top\n  generic map (\n    T => T\n  );\n"
    );
    let comp = component(&e, &Config::default()).unwrap();
    assert!(comp.contains("function f return T is <>"), "{comp}");
}

#[test]
fn invalid_patterns_are_reported() {
    let e = entity(COUNTER);
    let mut config = Config::default();
    config.generate.clock_pattern = "(".into();
    let err = testbench(&e, &config).unwrap_err();
    assert!(
        err.to_string()
            .starts_with("invalid generate.clock_pattern"),
        "{err}"
    );
}
