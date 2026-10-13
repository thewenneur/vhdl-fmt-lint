-- file: leaf.vhd
library ieee;
use ieee.std_logic_1164.all;
entity leaf is
  port (
    a : in std_logic;
    b : in std_logic := '0';
    pad_in : inout std_logic;
    pad_out : inout std_logic;
    pad_both : inout std_logic;
    y : out std_logic);
end entity;
architecture rtl of leaf is
begin
  y <= a and b and pad_in and pad_both;
  pad_out <= a;
  pad_both <= 'Z';
end architecture;
-- file: resets.vhd
library ieee;
use ieee.std_logic_1164.all;
entity resets is
  port (
    clk, rst, srst, rst_n, arst_n, en : in std_logic;
    d : in std_logic_vector(7 downto 0);
    q1, q2, q3, q4, q5, q6 : out std_logic_vector(7 downto 0);
    y1, y2 : out std_logic);
end entity;
architecture rtl of resets is
  type mem_t is array (0 to 3) of std_logic_vector(7 downto 0);
  signal mem : mem_t;
  signal data : std_logic_vector(7 downto 0);
begin
  -- Open inputs: `a` has no default, `b` has one.
  u1 : entity work.leaf port map (a => open, b => open, pad_in => y1, pad_out => y1,
                                  pad_both => y1, y => y2);
  u2 : entity work.leaf port map (open, open, y1, y1, y1, y2);

  -- Asynchronous reset; `data` is not reset, `mem` is a memory.
  async : process (clk, arst_n)
  begin
    if arst_n = '0' then
      q1 <= (others => '0');
    elsif rising_edge(clk) then
      q1 <= d;
      data <= d;
      mem(0) <= d;
    end if;
  end process;

  -- Synchronous reset on the same signal as another asynchronous one.
  sync : process (clk)
  begin
    if rising_edge(clk) then
      if arst_n = '0' then
        q2 <= (others => '0');
      else
        q2 <= data;
      end if;
    end if;
  end process;

  -- Active high here, active low below.
  high : process (clk, rst)
  begin
    if rst = '1' then
      q3 <= (others => '0');
    elsif rising_edge(clk) then
      q3 <= d;
    end if;
  end process;

  low : process (clk, rst)
  begin
    if rst = '0' then
      q4 <= (others => '0');
    elsif rising_edge(clk) then
      q4 <= d;
    end if;
  end process;

  -- Named active low, resets when high.
  named : process (clk)
  begin
    if rising_edge(clk) then
      if rst_n = '1' then
        q5 <= (others => '0');
      else
        q5 <= d;
      end if;
    end if;
  end process;

  -- A reset that overrides at the end: `q6` is not reset, by design.
  override : process (clk)
  begin
    if rising_edge(clk) then
      q6 <= d;
      if srst = '1' then
        null;
      end if;
    end if;
  end process;

  -- An enable is not a reset.
  enable : process (clk)
  begin
    if rising_edge(clk) then
      if en = '1' then
        mem(1) <= d;
      end if;
    end if;
  end process;
end architecture;
-- file: fsm.vhd
library ieee;
use ieee.std_logic_1164.all;
entity fsm is
  port (
    clk, rst, go, stop : in std_logic;
    busy : out std_logic);
end entity;
architecture rtl of fsm is
  type state_t is (IDLE, RUN, DONE, ORPHAN);
  signal state, next_state : state_t;
  type mode_t is (OFF, ON_A, ON_B);
  signal mode : mode_t := OFF;
  type free_t is (F0, F1, F2);
  signal free : free_t;
  signal other : free_t;
begin
  -- Two processes: ORPHAN is never reached, DONE is never left.
  reg : process (clk, rst)
  begin
    if rst = '1' then
      state <= IDLE;
    elsif rising_edge(clk) then
      state <= next_state;
    end if;
  end process;

  comb : process (all)
  begin
    next_state <= state;
    case state is
      when IDLE =>
        if go = '1' then
          next_state <= RUN;
        end if;
      when RUN =>
        if stop = '1' then
          next_state <= DONE;
        end if;
      when DONE => null;
      when ORPHAN => next_state <= IDLE;
    end case;
  end process;

  busy <= '1' when state = RUN else '0';

  -- One process with a variable-free signal: every state is reached and
  -- left.
  modes : process (clk)
  begin
    if rising_edge(clk) then
      case mode is
        when OFF => mode <= ON_A;
        when ON_A => mode <= ON_B;
        when others => mode <= OFF;
      end case;
    end if;
  end process;

  -- Assigned from another object: transitions unknown, nothing reported.
  frees : process (clk)
  begin
    if rising_edge(clk) then
      case free is
        when F0 => free <= other;
        when F1 => free <= F2;
        when F2 => null;
      end case;
      other <= free;
    end if;
  end process;
end architecture;
