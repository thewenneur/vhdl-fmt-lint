-- Declarations of commonly used Lattice iCE40 primitives, written for
-- vhdl-fmt-lint's name resolution from Lattice's public library reference.
-- Ports and the attributes most often set; what is not declared here is
-- not checked. Not for simulation or synthesis: those use the vendor's own
-- library.
library ieee;
use ieee.std_logic_1164.all;

package components is

  component SB_IO
    generic (
      NEG_TRIGGER : bit := '0';
      PIN_TYPE : bit_vector(5 downto 0) := "000000";
      PULLUP : bit := '0';
      IO_STANDARD : string := "SB_LVCMOS"
    );
    port (
      PACKAGE_PIN : inout std_logic;
      LATCH_INPUT_VALUE : in std_logic;
      CLOCK_ENABLE : in std_logic;
      INPUT_CLK : in std_logic;
      OUTPUT_CLK : in std_logic;
      OUTPUT_ENABLE : in std_logic;
      D_OUT_0 : in std_logic;
      D_OUT_1 : in std_logic;
      D_IN_0 : out std_logic;
      D_IN_1 : out std_logic
    );
  end component;

  component SB_GB
    port (
      USER_SIGNAL_TO_GLOBAL_BUFFER : in std_logic;
      GLOBAL_BUFFER_OUTPUT : out std_logic
    );
  end component;

  component SB_GB_IO
    generic (
      NEG_TRIGGER : bit := '0';
      PIN_TYPE : bit_vector(5 downto 0) := "000000";
      PULLUP : bit := '0';
      IO_STANDARD : string := "SB_LVCMOS"
    );
    port (
      PACKAGE_PIN : inout std_logic;
      GLOBAL_BUFFER_OUTPUT : out std_logic;
      LATCH_INPUT_VALUE : in std_logic;
      CLOCK_ENABLE : in std_logic;
      INPUT_CLK : in std_logic;
      OUTPUT_CLK : in std_logic;
      OUTPUT_ENABLE : in std_logic;
      D_OUT_0 : in std_logic;
      D_OUT_1 : in std_logic;
      D_IN_0 : out std_logic;
      D_IN_1 : out std_logic
    );
  end component;

  component SB_PLL40_CORE
    generic (
      FEEDBACK_PATH : string := "SIMPLE";
      DELAY_ADJUSTMENT_MODE_FEEDBACK : string := "FIXED";
      DELAY_ADJUSTMENT_MODE_RELATIVE : string := "FIXED";
      SHIFTREG_DIV_MODE : bit_vector(1 downto 0) := "00";
      FDA_FEEDBACK : bit_vector(3 downto 0) := "0000";
      FDA_RELATIVE : bit_vector(3 downto 0) := "0000";
      PLLOUT_SELECT : string := "GENCLK";
      DIVR : bit_vector(3 downto 0) := "0000";
      DIVF : bit_vector(6 downto 0) := "0000000";
      DIVQ : bit_vector(2 downto 0) := "000";
      FILTER_RANGE : bit_vector(2 downto 0) := "000";
      ENABLE_ICEGATE : bit := '0';
      TEST_MODE : bit := '0';
      EXTERNAL_DIVIDE_FACTOR : integer := 1
    );
    port (
      REFERENCECLK : in std_logic;
      PLLOUTCORE : out std_logic;
      PLLOUTGLOBAL : out std_logic;
      EXTFEEDBACK : in std_logic;
      DYNAMICDELAY : in std_logic_vector(7 downto 0);
      LOCK : out std_logic;
      BYPASS : in std_logic;
      RESETB : in std_logic;
      LATCHINPUTVALUE : in std_logic;
      SDO : out std_logic;
      SDI : in std_logic;
      SCLK : in std_logic
    );
  end component;

  component SB_RAM40_4K
    generic (
      WRITE_MODE : integer := 0;
      READ_MODE : integer := 0
    );
    port (
      RDATA : out std_logic_vector(15 downto 0);
      RCLK : in std_logic;
      RCLKE : in std_logic;
      RE : in std_logic;
      RADDR : in std_logic_vector(10 downto 0);
      WCLK : in std_logic;
      WCLKE : in std_logic;
      WE : in std_logic;
      WADDR : in std_logic_vector(10 downto 0);
      MASK : in std_logic_vector(15 downto 0);
      WDATA : in std_logic_vector(15 downto 0)
    );
  end component;

  component SB_HFOSC
    generic (
      CLKHF_DIV : string := "0b00"
    );
    port (
      CLKHFPU : in std_logic;
      CLKHFEN : in std_logic;
      CLKHF : out std_logic
    );
  end component;

  component SB_LFOSC
    port (
      CLKLFPU : in std_logic;
      CLKLFEN : in std_logic;
      CLKLF : out std_logic
    );
  end component;

  component SB_LUT4
    generic (
      LUT_INIT : bit_vector(15 downto 0) := X"0000"
    );
    port (
      O : out std_logic;
      I0 : in std_logic;
      I1 : in std_logic;
      I2 : in std_logic;
      I3 : in std_logic
    );
  end component;

  component SB_CARRY
    port (
      CO : out std_logic;
      I0 : in std_logic;
      I1 : in std_logic;
      CI : in std_logic
    );
  end component;

  component SB_DFF
    port (
      Q : out std_logic;
      C : in std_logic;
      D : in std_logic
    );
  end component;

  component SB_DFFE
    port (
      Q : out std_logic;
      C : in std_logic;
      E : in std_logic;
      D : in std_logic
    );
  end component;

  component SB_DFFR
    port (
      Q : out std_logic;
      C : in std_logic;
      R : in std_logic;
      D : in std_logic
    );
  end component;

  component SB_DFFS
    port (
      Q : out std_logic;
      C : in std_logic;
      S : in std_logic;
      D : in std_logic
    );
  end component;

  component SB_DFFSR
    port (
      Q : out std_logic;
      C : in std_logic;
      R : in std_logic;
      D : in std_logic
    );
  end component;

  component SB_DFFSS
    port (
      Q : out std_logic;
      C : in std_logic;
      S : in std_logic;
      D : in std_logic
    );
  end component;

  component SB_DFFER
    port (
      Q : out std_logic;
      C : in std_logic;
      E : in std_logic;
      R : in std_logic;
      D : in std_logic
    );
  end component;

  component SB_DFFES
    port (
      Q : out std_logic;
      C : in std_logic;
      E : in std_logic;
      S : in std_logic;
      D : in std_logic
    );
  end component;

end package components;
