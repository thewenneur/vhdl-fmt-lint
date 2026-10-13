-- A counter with the usual trimmings.
library ieee;
use ieee.std_logic_1164.all, ieee.numeric_std.all;

entity counter is
  generic (
    WIDTH : positive := 8;
    RESET_VALUE : natural := 0
  );
  port (
    clk, rst : in  std_logic;
    enable   : in  std_logic := '1';
    count    : out unsigned(WIDTH - 1 downto 0);
    done     : buffer std_logic
  );
begin
  assert WIDTH > 0 report "WIDTH must be positive" severity failure;
end entity counter;

architecture rtl of counter is
  signal value : unsigned(WIDTH - 1 downto 0) := (others => '0');
  constant MAX : natural := 2 ** WIDTH - 1;
  component adder is
    generic (N : natural);
    port (a, b : in unsigned(N - 1 downto 0); s : out unsigned(N - 1 downto 0));
  end component;
begin
  process (clk, rst) is
    variable next_value : unsigned(value'range);
  begin
    if rst = '1' then
      value <= to_unsigned(RESET_VALUE, WIDTH);
    elsif rising_edge(clk) then
      if enable = '1' then
        next_value := value + 1;
        value <= next_value after 1 ns;
      end if;
    end if;
  end process;

  count <= value;
  done  <= '1' when value = MAX else '0';

  u_add : adder
    generic map (N => WIDTH)
    port map (a => value, b => (others => '0'), s => open);

  u_direct : entity work.counter(rtl)
    generic map (WIDTH => 4)
    port map (clk => clk, rst => rst, count(3 downto 0) => open, done => open);

  u_plain : adder;
end architecture rtl;
