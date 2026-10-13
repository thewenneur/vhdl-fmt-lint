library ieee;
use ieee.std_logic_1164.all;
use ieee.numeric_std.all;

--! A synchronous first-in first-out buffer.
--!
--! Writes to a full FIFO and reads from an empty one are ignored.
entity fifo is
  generic (
    WIDTH : positive := 8; --! Bits per word
    DEPTH : positive := 16 --! Number of words
  );
  port (
    clk     : in  std_logic;                            --! Clock
    rst     : in  std_logic;                            --! Synchronous reset, active high
    wr_en   : in  std_logic;                            --! Write `wr_data` this cycle
    wr_data : in  std_logic_vector(WIDTH - 1 downto 0); --! Word to write
    rd_en   : in  std_logic;                            --! Take the oldest word
    rd_data : out std_logic_vector(WIDTH - 1 downto 0); --! Oldest word
    empty   : out std_logic;                            --! No word to read
    full    : out std_logic                             --! No room to write
  );
end entity fifo;

architecture rtl of fifo is

  type memory_t is array (0 to DEPTH - 1) of std_logic_vector(WIDTH - 1 downto 0);

  signal memory : memory_t;
  signal head   : natural range 0 to DEPTH - 1 := 0; --! Next word to write
  signal tail   : natural range 0 to DEPTH - 1 := 0; --! Next word to read
  signal count  : natural range 0 to DEPTH     := 0; --! Words stored

begin

  --! Moves the pointers and stores written words.
  pointers : process (clk) is
  begin
    if rising_edge(clk) then
      if rst = '1' then
        head  <= 0;
        tail  <= 0;
        count <= 0;
      else
        if wr_en = '1' and count < DEPTH then
          memory(head) <= wr_data;
          head         <= (head + 1) mod DEPTH;
        end if;
        if rd_en = '1' and count > 0 then
          tail <= (tail + 1) mod DEPTH;
        end if;
        if wr_en = '1' and count < DEPTH and not (rd_en = '1' and count > 0) then
          count <= count + 1;
        elsif rd_en = '1' and count > 0 and not (wr_en = '1' and count < DEPTH) then
          count <= count - 1;
        end if;
      end if;
    end if;
  end process pointers;

  rd_data <= memory(tail);
  empty   <= '1' when count = 0 else '0';
  full    <= '1' when count = DEPTH else '0';

end architecture rtl;
