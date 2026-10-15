-- Declarations of commonly used Lattice MachXO2 primitives, written for
-- vhdl-fmt-lint's name resolution from Lattice's public library reference.
-- Ports and the attributes most often set; what is not declared here is
-- not checked. Not for simulation or synthesis: those use the vendor's own
-- library.
library ieee;
use ieee.std_logic_1164.all;

package components is

  component IB
    port (
      I : in std_logic;
      O : out std_logic
    );
  end component;

  component OB
    port (
      I : in std_logic;
      O : out std_logic
    );
  end component;

  component OBZ
    port (
      I : in std_logic;
      T : in std_logic;
      O : out std_logic
    );
  end component;

  component BB
    port (
      I : in std_logic;
      T : in std_logic;
      O : out std_logic;
      B : inout std_logic
    );
  end component;

  component FD1S3AX
    generic (
      GSR : string := "ENABLED"
    );
    port (
      D : in std_logic;
      CK : in std_logic;
      Q : out std_logic
    );
  end component;

  component FD1S3BX
    generic (
      GSR : string := "ENABLED"
    );
    port (
      D : in std_logic;
      CK : in std_logic;
      PD : in std_logic;
      Q : out std_logic
    );
  end component;

  component FD1S3DX
    generic (
      GSR : string := "ENABLED"
    );
    port (
      D : in std_logic;
      CK : in std_logic;
      CD : in std_logic;
      Q : out std_logic
    );
  end component;

  component FD1P3AX
    generic (
      GSR : string := "ENABLED"
    );
    port (
      D : in std_logic;
      SP : in std_logic;
      CK : in std_logic;
      Q : out std_logic
    );
  end component;

  component FD1P3DX
    generic (
      GSR : string := "ENABLED"
    );
    port (
      D : in std_logic;
      SP : in std_logic;
      CK : in std_logic;
      CD : in std_logic;
      Q : out std_logic
    );
  end component;

  component DCCA
    port (
      CLKI : in std_logic;
      CE : in std_logic;
      CLKO : out std_logic
    );
  end component;

  component GSR
    port (
      GSR : in std_logic
    );
  end component;

  component PUR
    generic (
      RST_PULSE : integer := 1
    );
    port (
      PUR : in std_logic
    );
  end component;

  component VHI
    port (
      Z : out std_logic
    );
  end component;

  component VLO
    port (
      Z : out std_logic
    );
  end component;

  component OSCH
    generic (
      NOM_FREQ : string := "2.08"
    );
    port (
      STDBY : in std_logic;
      OSC : out std_logic;
      SEDSTDBY : out std_logic
    );
  end component;

  component EHXPLLJ
    generic (
      CLKI_DIV : integer := 1;
      CLKFB_DIV : integer := 1;
      CLKOP_DIV : integer := 8;
      CLKOS_DIV : integer := 8;
      CLKOS2_DIV : integer := 8;
      CLKOS3_DIV : integer := 8;
      CLKOP_ENABLE : string := "ENABLED";
      CLKOS_ENABLE : string := "DISABLED";
      CLKOS2_ENABLE : string := "DISABLED";
      CLKOS3_ENABLE : string := "DISABLED";
      FEEDBK_PATH : string := "CLKOP";
      PLLRST_ENA : string := "DISABLED";
      STDBY_ENABLE : string := "DISABLED";
      DPHASE_SOURCE : string := "DISABLED"
    );
    port (
      CLKI : in std_logic;
      CLKFB : in std_logic;
      PHASESEL1 : in std_logic;
      PHASESEL0 : in std_logic;
      PHASEDIR : in std_logic;
      PHASESTEP : in std_logic;
      LOADREG : in std_logic;
      STDBY : in std_logic;
      PLLWAKESYNC : in std_logic;
      RST : in std_logic;
      RESETM : in std_logic;
      RESETC : in std_logic;
      RESETD : in std_logic;
      ENCLKOP : in std_logic;
      ENCLKOS : in std_logic;
      ENCLKOS2 : in std_logic;
      ENCLKOS3 : in std_logic;
      PLLCLK : in std_logic;
      PLLRST : in std_logic;
      PLLSTB : in std_logic;
      PLLWE : in std_logic;
      PLLADDR : in std_logic_vector(4 downto 0);
      PLLDATI : in std_logic_vector(7 downto 0);
      CLKOP : out std_logic;
      CLKOS : out std_logic;
      CLKOS2 : out std_logic;
      CLKOS3 : out std_logic;
      LOCK : out std_logic;
      INTLOCK : out std_logic;
      REFCLK : out std_logic;
      PLLACK : out std_logic;
      PLLDATO : out std_logic_vector(7 downto 0)
    );
  end component;

  component ODDRXE
    generic (
      GSR : string := "ENABLED"
    );
    port (
      D0 : in std_logic;
      D1 : in std_logic;
      SCLK : in std_logic;
      RST : in std_logic;
      Q : out std_logic
    );
  end component;

  component IDDRXE
    generic (
      GSR : string := "ENABLED"
    );
    port (
      D : in std_logic;
      SCLK : in std_logic;
      RST : in std_logic;
      Q0 : out std_logic;
      Q1 : out std_logic
    );
  end component;

  component DP8KC
    generic (
      DATA_WIDTH_A : integer := 9;
      REGMODE_A : string := "NOREG";
      CSDECODE_A : string := "0b000";
      WRITEMODE_A : string := "NORMAL";
      DATA_WIDTH_B : integer := 9;
      REGMODE_B : string := "NOREG";
      CSDECODE_B : string := "0b000";
      WRITEMODE_B : string := "NORMAL";
      GSR : string := "ENABLED";
      RESETMODE : string := "SYNC";
      ASYNC_RESET_RELEASE : string := "SYNC";
      INIT_DATA : string := "STATIC"
    );
    port (
      DIA0 : in std_logic;
      DIA1 : in std_logic;
      DIA2 : in std_logic;
      DIA3 : in std_logic;
      DIA4 : in std_logic;
      DIA5 : in std_logic;
      DIA6 : in std_logic;
      DIA7 : in std_logic;
      DIA8 : in std_logic;
      ADA0 : in std_logic;
      ADA1 : in std_logic;
      ADA2 : in std_logic;
      ADA3 : in std_logic;
      ADA4 : in std_logic;
      ADA5 : in std_logic;
      ADA6 : in std_logic;
      ADA7 : in std_logic;
      ADA8 : in std_logic;
      ADA9 : in std_logic;
      ADA10 : in std_logic;
      ADA11 : in std_logic;
      ADA12 : in std_logic;
      CEA : in std_logic;
      OCEA : in std_logic;
      CLKA : in std_logic;
      WEA : in std_logic;
      CSA0 : in std_logic;
      CSA1 : in std_logic;
      CSA2 : in std_logic;
      RSTA : in std_logic;
      DIB0 : in std_logic;
      DIB1 : in std_logic;
      DIB2 : in std_logic;
      DIB3 : in std_logic;
      DIB4 : in std_logic;
      DIB5 : in std_logic;
      DIB6 : in std_logic;
      DIB7 : in std_logic;
      DIB8 : in std_logic;
      ADB0 : in std_logic;
      ADB1 : in std_logic;
      ADB2 : in std_logic;
      ADB3 : in std_logic;
      ADB4 : in std_logic;
      ADB5 : in std_logic;
      ADB6 : in std_logic;
      ADB7 : in std_logic;
      ADB8 : in std_logic;
      ADB9 : in std_logic;
      ADB10 : in std_logic;
      ADB11 : in std_logic;
      ADB12 : in std_logic;
      CEB : in std_logic;
      OCEB : in std_logic;
      CLKB : in std_logic;
      WEB : in std_logic;
      CSB0 : in std_logic;
      CSB1 : in std_logic;
      CSB2 : in std_logic;
      RSTB : in std_logic;
      DOA0 : out std_logic;
      DOA1 : out std_logic;
      DOA2 : out std_logic;
      DOA3 : out std_logic;
      DOA4 : out std_logic;
      DOA5 : out std_logic;
      DOA6 : out std_logic;
      DOA7 : out std_logic;
      DOA8 : out std_logic;
      DOB0 : out std_logic;
      DOB1 : out std_logic;
      DOB2 : out std_logic;
      DOB3 : out std_logic;
      DOB4 : out std_logic;
      DOB5 : out std_logic;
      DOB6 : out std_logic;
      DOB7 : out std_logic;
      DOB8 : out std_logic
    );
  end component;

end package components;
