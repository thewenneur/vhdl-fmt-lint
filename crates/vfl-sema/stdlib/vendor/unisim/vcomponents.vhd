-- Declarations of commonly used Xilinx UNISIM primitives (7 series and
-- later), written for vhdl-fmt-lint's name resolution from AMD's public
-- library guides (UG953, UG974). Ports and the generics most often set;
-- what is not declared here is not checked. Not for simulation or
-- synthesis: those use the vendor's own library.
library ieee;
use ieee.std_logic_1164.all;

package vcomponents is

  component BUFG
    port (
      O : out std_ulogic;
      I : in std_ulogic
    );
  end component;

  component BUFGCE
    port (
      O : out std_ulogic;
      CE : in std_ulogic;
      I : in std_ulogic
    );
  end component;

  component BUFGMUX
    generic (
      CLK_SEL_TYPE : string := "SYNC"
    );
    port (
      O : out std_ulogic;
      I0 : in std_ulogic;
      I1 : in std_ulogic;
      S : in std_ulogic
    );
  end component;

  component BUFGMUX_CTRL
    port (
      O : out std_ulogic;
      I0 : in std_ulogic;
      I1 : in std_ulogic;
      S : in std_ulogic
    );
  end component;

  component BUFH
    port (
      O : out std_ulogic;
      I : in std_ulogic
    );
  end component;

  component BUFIO
    port (
      O : out std_ulogic;
      I : in std_ulogic
    );
  end component;

  component BUFR
    generic (
      BUFR_DIVIDE : string := "BYPASS";
      SIM_DEVICE : string := "7SERIES"
    );
    port (
      O : out std_ulogic;
      CE : in std_ulogic;
      CLR : in std_ulogic;
      I : in std_ulogic
    );
  end component;

  component IBUF
    generic (
      IBUF_LOW_PWR : boolean := TRUE;
      IOSTANDARD : string := "DEFAULT"
    );
    port (
      O : out std_ulogic;
      I : in std_ulogic
    );
  end component;

  component IBUFG
    generic (
      IBUF_LOW_PWR : boolean := TRUE;
      IOSTANDARD : string := "DEFAULT"
    );
    port (
      O : out std_ulogic;
      I : in std_ulogic
    );
  end component;

  component OBUF
    generic (
      DRIVE : integer := 12;
      IOSTANDARD : string := "DEFAULT";
      SLEW : string := "SLOW"
    );
    port (
      O : out std_ulogic;
      I : in std_ulogic
    );
  end component;

  component OBUFT
    generic (
      DRIVE : integer := 12;
      IOSTANDARD : string := "DEFAULT";
      SLEW : string := "SLOW"
    );
    port (
      O : out std_ulogic;
      I : in std_ulogic;
      T : in std_ulogic
    );
  end component;

  component IOBUF
    generic (
      DRIVE : integer := 12;
      IBUF_LOW_PWR : boolean := TRUE;
      IOSTANDARD : string := "DEFAULT";
      SLEW : string := "SLOW"
    );
    port (
      O : out std_ulogic;
      IO : inout std_ulogic;
      I : in std_ulogic;
      T : in std_ulogic
    );
  end component;

  component IBUFDS
    generic (
      DIFF_TERM : boolean := FALSE;
      IBUF_LOW_PWR : boolean := TRUE;
      IOSTANDARD : string := "DEFAULT"
    );
    port (
      O : out std_ulogic;
      I : in std_ulogic;
      IB : in std_ulogic
    );
  end component;

  component IBUFGDS
    generic (
      DIFF_TERM : boolean := FALSE;
      IBUF_LOW_PWR : boolean := TRUE;
      IOSTANDARD : string := "DEFAULT"
    );
    port (
      O : out std_ulogic;
      I : in std_ulogic;
      IB : in std_ulogic
    );
  end component;

  component IBUFDS_GTE2
    port (
      O : out std_ulogic;
      ODIV2 : out std_ulogic;
      CEB : in std_ulogic;
      I : in std_ulogic;
      IB : in std_ulogic
    );
  end component;

  component OBUFDS
    generic (
      IOSTANDARD : string := "DEFAULT";
      SLEW : string := "SLOW"
    );
    port (
      O : out std_ulogic;
      OB : out std_ulogic;
      I : in std_ulogic
    );
  end component;

  component OBUFTDS
    generic (
      IOSTANDARD : string := "DEFAULT";
      SLEW : string := "SLOW"
    );
    port (
      O : out std_ulogic;
      OB : out std_ulogic;
      I : in std_ulogic;
      T : in std_ulogic
    );
  end component;

  component IOBUFDS
    generic (
      IOSTANDARD : string := "DEFAULT";
      SLEW : string := "SLOW"
    );
    port (
      O : out std_ulogic;
      IO : inout std_ulogic;
      IOB : inout std_ulogic;
      I : in std_ulogic;
      T : in std_ulogic
    );
  end component;

  component FDRE
    generic (
      INIT : bit := '0'
    );
    port (
      Q : out std_ulogic;
      C : in std_ulogic;
      CE : in std_ulogic;
      D : in std_ulogic;
      R : in std_ulogic
    );
  end component;

  component FDSE
    generic (
      INIT : bit := '1'
    );
    port (
      Q : out std_ulogic;
      C : in std_ulogic;
      CE : in std_ulogic;
      D : in std_ulogic;
      S : in std_ulogic
    );
  end component;

  component FDCE
    generic (
      INIT : bit := '0'
    );
    port (
      Q : out std_ulogic;
      C : in std_ulogic;
      CE : in std_ulogic;
      CLR : in std_ulogic;
      D : in std_ulogic
    );
  end component;

  component FDPE
    generic (
      INIT : bit := '1'
    );
    port (
      Q : out std_ulogic;
      C : in std_ulogic;
      CE : in std_ulogic;
      D : in std_ulogic;
      PRE : in std_ulogic
    );
  end component;

  component LUT1
    generic (
      INIT : bit_vector(1 downto 0) := "00"
    );
    port (
      O : out std_ulogic;
      I0 : in std_ulogic
    );
  end component;

  component LUT2
    generic (
      INIT : bit_vector(3 downto 0) := X"0"
    );
    port (
      O : out std_ulogic;
      I0 : in std_ulogic;
      I1 : in std_ulogic
    );
  end component;

  component LUT3
    generic (
      INIT : bit_vector(7 downto 0) := X"00"
    );
    port (
      O : out std_ulogic;
      I0 : in std_ulogic;
      I1 : in std_ulogic;
      I2 : in std_ulogic
    );
  end component;

  component LUT4
    generic (
      INIT : bit_vector(15 downto 0) := X"0000"
    );
    port (
      O : out std_ulogic;
      I0 : in std_ulogic;
      I1 : in std_ulogic;
      I2 : in std_ulogic;
      I3 : in std_ulogic
    );
  end component;

  component LUT5
    generic (
      INIT : bit_vector(31 downto 0) := X"00000000"
    );
    port (
      O : out std_ulogic;
      I0 : in std_ulogic;
      I1 : in std_ulogic;
      I2 : in std_ulogic;
      I3 : in std_ulogic;
      I4 : in std_ulogic
    );
  end component;

  component LUT6
    generic (
      INIT : bit_vector(63 downto 0) := X"0000000000000000"
    );
    port (
      O : out std_ulogic;
      I0 : in std_ulogic;
      I1 : in std_ulogic;
      I2 : in std_ulogic;
      I3 : in std_ulogic;
      I4 : in std_ulogic;
      I5 : in std_ulogic
    );
  end component;

  component SRL16E
    generic (
      INIT : bit_vector(15 downto 0) := X"0000"
    );
    port (
      Q : out std_ulogic;
      A0 : in std_ulogic;
      A1 : in std_ulogic;
      A2 : in std_ulogic;
      A3 : in std_ulogic;
      CE : in std_ulogic;
      CLK : in std_ulogic;
      D : in std_ulogic
    );
  end component;

  component SRLC32E
    generic (
      INIT : bit_vector(31 downto 0) := X"00000000"
    );
    port (
      Q : out std_ulogic;
      Q31 : out std_ulogic;
      A : in std_logic_vector(4 downto 0);
      CE : in std_ulogic;
      CLK : in std_ulogic;
      D : in std_ulogic
    );
  end component;

  component CARRY4
    port (
      CO : out std_logic_vector(3 downto 0);
      O : out std_logic_vector(3 downto 0);
      CI : in std_ulogic;
      CYINIT : in std_ulogic;
      DI : in std_logic_vector(3 downto 0);
      S : in std_logic_vector(3 downto 0)
    );
  end component;

  component MUXF7
    port (
      O : out std_ulogic;
      I0 : in std_ulogic;
      I1 : in std_ulogic;
      S : in std_ulogic
    );
  end component;

  component MUXF8
    port (
      O : out std_ulogic;
      I0 : in std_ulogic;
      I1 : in std_ulogic;
      S : in std_ulogic
    );
  end component;

  component ODDR
    generic (
      DDR_CLK_EDGE : string := "OPPOSITE_EDGE";
      INIT : bit := '0';
      SRTYPE : string := "SYNC"
    );
    port (
      Q : out std_ulogic;
      C : in std_ulogic;
      CE : in std_ulogic;
      D1 : in std_ulogic;
      D2 : in std_ulogic;
      R : in std_ulogic;
      S : in std_ulogic
    );
  end component;

  component IDDR
    generic (
      DDR_CLK_EDGE : string := "OPPOSITE_EDGE";
      INIT_Q1 : bit := '0';
      INIT_Q2 : bit := '0';
      SRTYPE : string := "SYNC"
    );
    port (
      Q1 : out std_ulogic;
      Q2 : out std_ulogic;
      C : in std_ulogic;
      CE : in std_ulogic;
      D : in std_ulogic;
      R : in std_ulogic;
      S : in std_ulogic
    );
  end component;

  component IDELAYCTRL
    port (
      RDY : out std_ulogic;
      REFCLK : in std_ulogic;
      RST : in std_ulogic
    );
  end component;

  component IDELAYE2
    generic (
      CINVCTRL_SEL : string := "FALSE";
      DELAY_SRC : string := "IDATAIN";
      HIGH_PERFORMANCE_MODE : string := "FALSE";
      IDELAY_TYPE : string := "FIXED";
      IDELAY_VALUE : integer := 0;
      PIPE_SEL : string := "FALSE";
      REFCLK_FREQUENCY : real := 200.0;
      SIGNAL_PATTERN : string := "DATA"
    );
    port (
      CNTVALUEOUT : out std_logic_vector(4 downto 0);
      DATAOUT : out std_ulogic;
      C : in std_ulogic;
      CE : in std_ulogic;
      CINVCTRL : in std_ulogic;
      CNTVALUEIN : in std_logic_vector(4 downto 0);
      DATAIN : in std_ulogic;
      IDATAIN : in std_ulogic;
      INC : in std_ulogic;
      LD : in std_ulogic;
      LDPIPEEN : in std_ulogic;
      REGRST : in std_ulogic
    );
  end component;

  component ODELAYE2
    generic (
      CINVCTRL_SEL : string := "FALSE";
      DELAY_SRC : string := "ODATAIN";
      HIGH_PERFORMANCE_MODE : string := "FALSE";
      ODELAY_TYPE : string := "FIXED";
      ODELAY_VALUE : integer := 0;
      PIPE_SEL : string := "FALSE";
      REFCLK_FREQUENCY : real := 200.0;
      SIGNAL_PATTERN : string := "DATA"
    );
    port (
      CNTVALUEOUT : out std_logic_vector(4 downto 0);
      DATAOUT : out std_ulogic;
      C : in std_ulogic;
      CE : in std_ulogic;
      CINVCTRL : in std_ulogic;
      CLKIN : in std_ulogic;
      CNTVALUEIN : in std_logic_vector(4 downto 0);
      INC : in std_ulogic;
      LD : in std_ulogic;
      LDPIPEEN : in std_ulogic;
      ODATAIN : in std_ulogic;
      REGRST : in std_ulogic
    );
  end component;

  component OSERDESE2
    generic (
      DATA_RATE_OQ : string := "DDR";
      DATA_RATE_TQ : string := "DDR";
      DATA_WIDTH : integer := 4;
      INIT_OQ : bit := '0';
      INIT_TQ : bit := '0';
      SERDES_MODE : string := "MASTER";
      SRVAL_OQ : bit := '0';
      SRVAL_TQ : bit := '0';
      TBYTE_CTL : string := "FALSE";
      TBYTE_SRC : string := "FALSE";
      TRISTATE_WIDTH : integer := 4
    );
    port (
      OFB : out std_ulogic;
      OQ : out std_ulogic;
      SHIFTOUT1 : out std_ulogic;
      SHIFTOUT2 : out std_ulogic;
      TBYTEOUT : out std_ulogic;
      TFB : out std_ulogic;
      TQ : out std_ulogic;
      CLK : in std_ulogic;
      CLKDIV : in std_ulogic;
      D1 : in std_ulogic;
      D2 : in std_ulogic;
      D3 : in std_ulogic;
      D4 : in std_ulogic;
      D5 : in std_ulogic;
      D6 : in std_ulogic;
      D7 : in std_ulogic;
      D8 : in std_ulogic;
      OCE : in std_ulogic;
      RST : in std_ulogic;
      SHIFTIN1 : in std_ulogic;
      SHIFTIN2 : in std_ulogic;
      T1 : in std_ulogic;
      T2 : in std_ulogic;
      T3 : in std_ulogic;
      T4 : in std_ulogic;
      TBYTEIN : in std_ulogic;
      TCE : in std_ulogic
    );
  end component;

  component ISERDESE2
    generic (
      DATA_RATE : string := "DDR";
      DATA_WIDTH : integer := 4;
      DYN_CLKDIV_INV_EN : string := "FALSE";
      DYN_CLK_INV_EN : string := "FALSE";
      INIT_Q1 : bit := '0';
      INIT_Q2 : bit := '0';
      INIT_Q3 : bit := '0';
      INIT_Q4 : bit := '0';
      INTERFACE_TYPE : string := "MEMORY";
      IOBDELAY : string := "NONE";
      NUM_CE : integer := 2;
      OFB_USED : string := "FALSE";
      SERDES_MODE : string := "MASTER";
      SRVAL_Q1 : bit := '0';
      SRVAL_Q2 : bit := '0';
      SRVAL_Q3 : bit := '0';
      SRVAL_Q4 : bit := '0'
    );
    port (
      O : out std_ulogic;
      Q1 : out std_ulogic;
      Q2 : out std_ulogic;
      Q3 : out std_ulogic;
      Q4 : out std_ulogic;
      Q5 : out std_ulogic;
      Q6 : out std_ulogic;
      Q7 : out std_ulogic;
      Q8 : out std_ulogic;
      SHIFTOUT1 : out std_ulogic;
      SHIFTOUT2 : out std_ulogic;
      BITSLIP : in std_ulogic;
      CE1 : in std_ulogic;
      CE2 : in std_ulogic;
      CLK : in std_ulogic;
      CLKB : in std_ulogic;
      CLKDIV : in std_ulogic;
      CLKDIVP : in std_ulogic;
      D : in std_ulogic;
      DDLY : in std_ulogic;
      DYNCLKDIVSEL : in std_ulogic;
      DYNCLKSEL : in std_ulogic;
      OCLK : in std_ulogic;
      OCLKB : in std_ulogic;
      OFB : in std_ulogic;
      RST : in std_ulogic;
      SHIFTIN1 : in std_ulogic;
      SHIFTIN2 : in std_ulogic
    );
  end component;

  component MMCME2_BASE
    generic (
      BANDWIDTH : string := "OPTIMIZED";
      CLKFBOUT_PHASE : real := 0.0;
      CLKIN1_PERIOD : real := 0.0;
      DIVCLK_DIVIDE : integer := 1;
      REF_JITTER1 : real := 0.0;
      CLKFBOUT_MULT_F : real := 5.0;
      CLKOUT0_DIVIDE_F : real := 1.0;
      CLKOUT0_DUTY_CYCLE : real := 0.5;
      CLKOUT0_PHASE : real := 0.0;
      CLKOUT1_DIVIDE : integer := 1;
      CLKOUT1_DUTY_CYCLE : real := 0.5;
      CLKOUT1_PHASE : real := 0.0;
      CLKOUT2_DIVIDE : integer := 1;
      CLKOUT2_DUTY_CYCLE : real := 0.5;
      CLKOUT2_PHASE : real := 0.0;
      CLKOUT3_DIVIDE : integer := 1;
      CLKOUT3_DUTY_CYCLE : real := 0.5;
      CLKOUT3_PHASE : real := 0.0;
      CLKOUT4_DIVIDE : integer := 1;
      CLKOUT4_DUTY_CYCLE : real := 0.5;
      CLKOUT4_PHASE : real := 0.0;
      CLKOUT5_DIVIDE : integer := 1;
      CLKOUT5_DUTY_CYCLE : real := 0.5;
      CLKOUT5_PHASE : real := 0.0;
      CLKOUT6_DIVIDE : integer := 1;
      CLKOUT6_DUTY_CYCLE : real := 0.5;
      CLKOUT6_PHASE : real := 0.0
    );
    port (
      CLKFBOUT : out std_ulogic;
      CLKFBOUTB : out std_ulogic;
      CLKOUT0 : out std_ulogic;
      CLKOUT0B : out std_ulogic;
      CLKOUT1 : out std_ulogic;
      CLKOUT1B : out std_ulogic;
      CLKOUT2 : out std_ulogic;
      CLKOUT2B : out std_ulogic;
      CLKOUT3 : out std_ulogic;
      CLKOUT3B : out std_ulogic;
      CLKOUT4 : out std_ulogic;
      CLKOUT5 : out std_ulogic;
      CLKOUT6 : out std_ulogic;
      LOCKED : out std_ulogic;
      CLKFBIN : in std_ulogic;
      CLKIN1 : in std_ulogic;
      PWRDWN : in std_ulogic;
      RST : in std_ulogic
    );
  end component;

  component MMCME2_ADV
    generic (
      BANDWIDTH : string := "OPTIMIZED";
      CLKFBOUT_PHASE : real := 0.0;
      CLKIN1_PERIOD : real := 0.0;
      DIVCLK_DIVIDE : integer := 1;
      REF_JITTER1 : real := 0.0;
      CLKFBOUT_MULT_F : real := 5.0;
      CLKOUT0_DIVIDE_F : real := 1.0;
      CLKOUT0_DUTY_CYCLE : real := 0.5;
      CLKOUT0_PHASE : real := 0.0;
      CLKOUT1_DIVIDE : integer := 1;
      CLKOUT1_DUTY_CYCLE : real := 0.5;
      CLKOUT1_PHASE : real := 0.0;
      CLKOUT2_DIVIDE : integer := 1;
      CLKOUT2_DUTY_CYCLE : real := 0.5;
      CLKOUT2_PHASE : real := 0.0;
      CLKOUT3_DIVIDE : integer := 1;
      CLKOUT3_DUTY_CYCLE : real := 0.5;
      CLKOUT3_PHASE : real := 0.0;
      CLKOUT4_DIVIDE : integer := 1;
      CLKOUT4_DUTY_CYCLE : real := 0.5;
      CLKOUT4_PHASE : real := 0.0;
      CLKOUT5_DIVIDE : integer := 1;
      CLKOUT5_DUTY_CYCLE : real := 0.5;
      CLKOUT5_PHASE : real := 0.0;
      CLKOUT6_DIVIDE : integer := 1;
      CLKOUT6_DUTY_CYCLE : real := 0.5;
      CLKOUT6_PHASE : real := 0.0;
      CLKIN2_PERIOD : real := 0.0;
      COMPENSATION : string := "ZHOLD";
      REF_JITTER2 : real := 0.0
    );
    port (
      CLKFBOUT : out std_ulogic;
      CLKFBOUTB : out std_ulogic;
      CLKFBSTOPPED : out std_ulogic;
      CLKINSTOPPED : out std_ulogic;
      CLKOUT0 : out std_ulogic;
      CLKOUT0B : out std_ulogic;
      CLKOUT1 : out std_ulogic;
      CLKOUT1B : out std_ulogic;
      CLKOUT2 : out std_ulogic;
      CLKOUT2B : out std_ulogic;
      CLKOUT3 : out std_ulogic;
      CLKOUT3B : out std_ulogic;
      CLKOUT4 : out std_ulogic;
      CLKOUT5 : out std_ulogic;
      CLKOUT6 : out std_ulogic;
      DO : out std_logic_vector(15 downto 0);
      DRDY : out std_ulogic;
      LOCKED : out std_ulogic;
      PSDONE : out std_ulogic;
      CLKFBIN : in std_ulogic;
      CLKIN1 : in std_ulogic;
      CLKIN2 : in std_ulogic;
      CLKINSEL : in std_ulogic;
      DADDR : in std_logic_vector(6 downto 0);
      DCLK : in std_ulogic;
      DEN : in std_ulogic;
      DI : in std_logic_vector(15 downto 0);
      DWE : in std_ulogic;
      PSCLK : in std_ulogic;
      PSEN : in std_ulogic;
      PSINCDEC : in std_ulogic;
      PWRDWN : in std_ulogic;
      RST : in std_ulogic
    );
  end component;

  component PLLE2_BASE
    generic (
      BANDWIDTH : string := "OPTIMIZED";
      CLKFBOUT_PHASE : real := 0.0;
      CLKIN1_PERIOD : real := 0.0;
      DIVCLK_DIVIDE : integer := 1;
      REF_JITTER1 : real := 0.0;
      CLKFBOUT_MULT : integer := 5;
      CLKOUT0_DIVIDE : integer := 1;
      CLKOUT0_DUTY_CYCLE : real := 0.5;
      CLKOUT0_PHASE : real := 0.0;
      CLKOUT1_DIVIDE : integer := 1;
      CLKOUT1_DUTY_CYCLE : real := 0.5;
      CLKOUT1_PHASE : real := 0.0;
      CLKOUT2_DIVIDE : integer := 1;
      CLKOUT2_DUTY_CYCLE : real := 0.5;
      CLKOUT2_PHASE : real := 0.0;
      CLKOUT3_DIVIDE : integer := 1;
      CLKOUT3_DUTY_CYCLE : real := 0.5;
      CLKOUT3_PHASE : real := 0.0;
      CLKOUT4_DIVIDE : integer := 1;
      CLKOUT4_DUTY_CYCLE : real := 0.5;
      CLKOUT4_PHASE : real := 0.0;
      CLKOUT5_DIVIDE : integer := 1;
      CLKOUT5_DUTY_CYCLE : real := 0.5;
      CLKOUT5_PHASE : real := 0.0
    );
    port (
      CLKFBOUT : out std_ulogic;
      CLKOUT0 : out std_ulogic;
      CLKOUT1 : out std_ulogic;
      CLKOUT2 : out std_ulogic;
      CLKOUT3 : out std_ulogic;
      CLKOUT4 : out std_ulogic;
      CLKOUT5 : out std_ulogic;
      LOCKED : out std_ulogic;
      CLKFBIN : in std_ulogic;
      CLKIN1 : in std_ulogic;
      PWRDWN : in std_ulogic;
      RST : in std_ulogic
    );
  end component;

  component PLLE2_ADV
    generic (
      BANDWIDTH : string := "OPTIMIZED";
      CLKFBOUT_PHASE : real := 0.0;
      CLKIN1_PERIOD : real := 0.0;
      DIVCLK_DIVIDE : integer := 1;
      REF_JITTER1 : real := 0.0;
      CLKFBOUT_MULT : integer := 5;
      CLKOUT0_DIVIDE : integer := 1;
      CLKOUT0_DUTY_CYCLE : real := 0.5;
      CLKOUT0_PHASE : real := 0.0;
      CLKOUT1_DIVIDE : integer := 1;
      CLKOUT1_DUTY_CYCLE : real := 0.5;
      CLKOUT1_PHASE : real := 0.0;
      CLKOUT2_DIVIDE : integer := 1;
      CLKOUT2_DUTY_CYCLE : real := 0.5;
      CLKOUT2_PHASE : real := 0.0;
      CLKOUT3_DIVIDE : integer := 1;
      CLKOUT3_DUTY_CYCLE : real := 0.5;
      CLKOUT3_PHASE : real := 0.0;
      CLKOUT4_DIVIDE : integer := 1;
      CLKOUT4_DUTY_CYCLE : real := 0.5;
      CLKOUT4_PHASE : real := 0.0;
      CLKOUT5_DIVIDE : integer := 1;
      CLKOUT5_DUTY_CYCLE : real := 0.5;
      CLKOUT5_PHASE : real := 0.0;
      CLKIN2_PERIOD : real := 0.0;
      COMPENSATION : string := "ZHOLD";
      REF_JITTER2 : real := 0.0
    );
    port (
      CLKFBOUT : out std_ulogic;
      CLKOUT0 : out std_ulogic;
      CLKOUT1 : out std_ulogic;
      CLKOUT2 : out std_ulogic;
      CLKOUT3 : out std_ulogic;
      CLKOUT4 : out std_ulogic;
      CLKOUT5 : out std_ulogic;
      DO : out std_logic_vector(15 downto 0);
      DRDY : out std_ulogic;
      LOCKED : out std_ulogic;
      CLKFBIN : in std_ulogic;
      CLKIN1 : in std_ulogic;
      CLKIN2 : in std_ulogic;
      CLKINSEL : in std_ulogic;
      DADDR : in std_logic_vector(6 downto 0);
      DCLK : in std_ulogic;
      DEN : in std_ulogic;
      DI : in std_logic_vector(15 downto 0);
      DWE : in std_ulogic;
      PWRDWN : in std_ulogic;
      RST : in std_ulogic
    );
  end component;

  component RAMB36E1
    generic (
      DOA_REG : integer := 0;
      DOB_REG : integer := 0;
      INIT_A : bit_vector(35 downto 0) := (others => '0');
      INIT_B : bit_vector(35 downto 0) := (others => '0');
      RAM_MODE : string := "TDP";
      READ_WIDTH_A : integer := 0;
      READ_WIDTH_B : integer := 0;
      SIM_COLLISION_CHECK : string := "ALL";
      SIM_DEVICE : string := "7SERIES";
      SRVAL_A : bit_vector(35 downto 0) := (others => '0');
      SRVAL_B : bit_vector(35 downto 0) := (others => '0');
      WRITE_MODE_A : string := "WRITE_FIRST";
      WRITE_MODE_B : string := "WRITE_FIRST";
      WRITE_WIDTH_A : integer := 0;
      WRITE_WIDTH_B : integer := 0;
      EN_ECC_READ : boolean := FALSE;
      EN_ECC_WRITE : boolean := FALSE;
      RAM_EXTENSION_A : string := "NONE";
      RAM_EXTENSION_B : string := "NONE";
      RDADDR_COLLISION_HWCONFIG : string := "DELAYED_WRITE"
    );
    port (
      CASCADEOUTA : out std_ulogic;
      CASCADEOUTB : out std_ulogic;
      DBITERR : out std_ulogic;
      DOADO : out std_logic_vector(31 downto 0);
      DOBDO : out std_logic_vector(31 downto 0);
      DOPADOP : out std_logic_vector(3 downto 0);
      DOPBDOP : out std_logic_vector(3 downto 0);
      ECCPARITY : out std_logic_vector(7 downto 0);
      RDADDRECC : out std_logic_vector(8 downto 0);
      SBITERR : out std_ulogic;
      ADDRARDADDR : in std_logic_vector(15 downto 0);
      ADDRBWRADDR : in std_logic_vector(15 downto 0);
      CASCADEINA : in std_ulogic;
      CASCADEINB : in std_ulogic;
      CLKARDCLK : in std_ulogic;
      CLKBWRCLK : in std_ulogic;
      DIADI : in std_logic_vector(31 downto 0);
      DIBDI : in std_logic_vector(31 downto 0);
      DIPADIP : in std_logic_vector(3 downto 0);
      DIPBDIP : in std_logic_vector(3 downto 0);
      ENARDEN : in std_ulogic;
      ENBWREN : in std_ulogic;
      INJECTDBITERR : in std_ulogic;
      INJECTSBITERR : in std_ulogic;
      REGCEAREGCE : in std_ulogic;
      REGCEB : in std_ulogic;
      RSTRAMARSTRAM : in std_ulogic;
      RSTRAMB : in std_ulogic;
      RSTREGARSTREG : in std_ulogic;
      RSTREGB : in std_ulogic;
      WEA : in std_logic_vector(3 downto 0);
      WEBWE : in std_logic_vector(7 downto 0)
    );
  end component;

  component RAMB18E1
    generic (
      DOA_REG : integer := 0;
      DOB_REG : integer := 0;
      INIT_A : bit_vector(17 downto 0) := (others => '0');
      INIT_B : bit_vector(17 downto 0) := (others => '0');
      RAM_MODE : string := "TDP";
      READ_WIDTH_A : integer := 0;
      READ_WIDTH_B : integer := 0;
      SIM_COLLISION_CHECK : string := "ALL";
      SIM_DEVICE : string := "7SERIES";
      SRVAL_A : bit_vector(17 downto 0) := (others => '0');
      SRVAL_B : bit_vector(17 downto 0) := (others => '0');
      WRITE_MODE_A : string := "WRITE_FIRST";
      WRITE_MODE_B : string := "WRITE_FIRST";
      WRITE_WIDTH_A : integer := 0;
      WRITE_WIDTH_B : integer := 0
    );
    port (
      DOADO : out std_logic_vector(15 downto 0);
      DOBDO : out std_logic_vector(15 downto 0);
      DOPADOP : out std_logic_vector(1 downto 0);
      DOPBDOP : out std_logic_vector(1 downto 0);
      ADDRARDADDR : in std_logic_vector(13 downto 0);
      ADDRBWRADDR : in std_logic_vector(13 downto 0);
      CLKARDCLK : in std_ulogic;
      CLKBWRCLK : in std_ulogic;
      DIADI : in std_logic_vector(15 downto 0);
      DIBDI : in std_logic_vector(15 downto 0);
      DIPADIP : in std_logic_vector(1 downto 0);
      DIPBDIP : in std_logic_vector(1 downto 0);
      ENARDEN : in std_ulogic;
      ENBWREN : in std_ulogic;
      REGCEAREGCE : in std_ulogic;
      REGCEB : in std_ulogic;
      RSTRAMARSTRAM : in std_ulogic;
      RSTRAMB : in std_ulogic;
      RSTREGARSTREG : in std_ulogic;
      RSTREGB : in std_ulogic;
      WEA : in std_logic_vector(1 downto 0);
      WEBWE : in std_logic_vector(3 downto 0)
    );
  end component;

  component FIFO36E1
    generic (
      ALMOST_EMPTY_OFFSET : bit_vector(12 downto 0) := (others => '0');
      ALMOST_FULL_OFFSET : bit_vector(12 downto 0) := (others => '0');
      DATA_WIDTH : integer := 4;
      DO_REG : integer := 1;
      EN_SYN : boolean := FALSE;
      FIFO_MODE : string := "FIFO36";
      FIRST_WORD_FALL_THROUGH : boolean := FALSE;
      SIM_DEVICE : string := "7SERIES"
    );
    port (
      ALMOSTEMPTY : out std_ulogic;
      ALMOSTFULL : out std_ulogic;
      DBITERR : out std_ulogic;
      DO : out std_logic_vector(63 downto 0);
      DOP : out std_logic_vector(7 downto 0);
      ECCPARITY : out std_logic_vector(7 downto 0);
      EMPTY : out std_ulogic;
      FULL : out std_ulogic;
      RDCOUNT : out std_logic_vector(12 downto 0);
      RDERR : out std_ulogic;
      SBITERR : out std_ulogic;
      WRCOUNT : out std_logic_vector(12 downto 0);
      WRERR : out std_ulogic;
      DI : in std_logic_vector(63 downto 0);
      DIP : in std_logic_vector(7 downto 0);
      INJECTDBITERR : in std_ulogic;
      INJECTSBITERR : in std_ulogic;
      RDCLK : in std_ulogic;
      RDEN : in std_ulogic;
      REGCE : in std_ulogic;
      RST : in std_ulogic;
      RSTREG : in std_ulogic;
      WRCLK : in std_ulogic;
      WREN : in std_ulogic
    );
  end component;

  component DSP48E1
    generic (
      ACASCREG : integer := 1;
      ADREG : integer := 1;
      ALUMODEREG : integer := 1;
      AREG : integer := 1;
      BCASCREG : integer := 1;
      BREG : integer := 1;
      CARRYINREG : integer := 1;
      CARRYINSELREG : integer := 1;
      CREG : integer := 1;
      DREG : integer := 1;
      INMODEREG : integer := 1;
      MREG : integer := 1;
      OPMODEREG : integer := 1;
      PREG : integer := 1;
      A_INPUT : string := "DIRECT";
      AUTORESET_PATDET : string := "NO_RESET";
      B_INPUT : string := "DIRECT";
      MASK : bit_vector(47 downto 0) := X"3FFFFFFFFFFF";
      PATTERN : bit_vector(47 downto 0) := X"000000000000";
      SEL_MASK : string := "MASK";
      SEL_PATTERN : string := "PATTERN";
      USE_DPORT : boolean := FALSE;
      USE_MULT : string := "MULTIPLY";
      USE_PATTERN_DETECT : string := "NO_PATDET";
      USE_SIMD : string := "ONE48"
    );
    port (
      ACOUT : out std_logic_vector(29 downto 0);
      BCOUT : out std_logic_vector(17 downto 0);
      CARRYCASCOUT : out std_ulogic;
      CARRYOUT : out std_logic_vector(3 downto 0);
      MULTSIGNOUT : out std_ulogic;
      OVERFLOW : out std_ulogic;
      P : out std_logic_vector(47 downto 0);
      PATTERNBDETECT : out std_ulogic;
      PATTERNDETECT : out std_ulogic;
      PCOUT : out std_logic_vector(47 downto 0);
      UNDERFLOW : out std_ulogic;
      A : in std_logic_vector(29 downto 0);
      ACIN : in std_logic_vector(29 downto 0);
      ALUMODE : in std_logic_vector(3 downto 0);
      B : in std_logic_vector(17 downto 0);
      BCIN : in std_logic_vector(17 downto 0);
      C : in std_logic_vector(47 downto 0);
      CARRYCASCIN : in std_ulogic;
      CARRYIN : in std_ulogic;
      CARRYINSEL : in std_logic_vector(2 downto 0);
      CEA1 : in std_ulogic;
      CEA2 : in std_ulogic;
      CEAD : in std_ulogic;
      CEALUMODE : in std_ulogic;
      CEB1 : in std_ulogic;
      CEB2 : in std_ulogic;
      CEC : in std_ulogic;
      CECARRYIN : in std_ulogic;
      CECTRL : in std_ulogic;
      CED : in std_ulogic;
      CEINMODE : in std_ulogic;
      CEM : in std_ulogic;
      CEP : in std_ulogic;
      CLK : in std_ulogic;
      D : in std_logic_vector(24 downto 0);
      INMODE : in std_logic_vector(4 downto 0);
      MULTSIGNIN : in std_ulogic;
      OPMODE : in std_logic_vector(6 downto 0);
      PCIN : in std_logic_vector(47 downto 0);
      RSTA : in std_ulogic;
      RSTALLCARRYIN : in std_ulogic;
      RSTALUMODE : in std_ulogic;
      RSTB : in std_ulogic;
      RSTC : in std_ulogic;
      RSTCTRL : in std_ulogic;
      RSTD : in std_ulogic;
      RSTINMODE : in std_ulogic;
      RSTM : in std_ulogic;
      RSTP : in std_ulogic
    );
  end component;

  component STARTUPE2
    generic (
      PROG_USR : string := "FALSE";
      SIM_CCLK_FREQ : real := 0.0
    );
    port (
      CFGCLK : out std_ulogic;
      CFGMCLK : out std_ulogic;
      EOS : out std_ulogic;
      PREQ : out std_ulogic;
      CLK : in std_ulogic;
      GSR : in std_ulogic;
      GTS : in std_ulogic;
      KEYCLEARB : in std_ulogic;
      PACK : in std_ulogic;
      USRCCLKO : in std_ulogic;
      USRCCLKTS : in std_ulogic;
      USRDONEO : in std_ulogic;
      USRDONETS : in std_ulogic
    );
  end component;

  component DNA_PORT
    generic (
      SIM_DNA_VALUE : bit_vector(56 downto 0) := (others => '0')
    );
    port (
      DOUT : out std_ulogic;
      CLK : in std_ulogic;
      DIN : in std_ulogic;
      READ : in std_ulogic;
      SHIFT : in std_ulogic
    );
  end component;

  component GND
    port (
      G : out std_ulogic
    );
  end component;

  component VCC
    port (
      P : out std_ulogic
    );
  end component;

  component PULLUP
    port (
      O : out std_ulogic
    );
  end component;

  component PULLDOWN
    port (
      O : out std_ulogic
    );
  end component;

end package vcomponents;
