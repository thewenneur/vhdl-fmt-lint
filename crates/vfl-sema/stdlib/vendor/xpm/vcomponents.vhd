-- Declarations of the Xilinx Parameterized Macros (XPM) for CDC, FIFOs and
-- memories, written for vhdl-fmt-lint's name resolution from AMD's public
-- library guide (UG974). Not for simulation or synthesis: those use the
-- vendor's own library.
library ieee;
use ieee.std_logic_1164.all;

package vcomponents is

  component xpm_cdc_single
    generic (
      DEST_SYNC_FF : integer := 4;
      INIT_SYNC_FF : integer := 0;
      SIM_ASSERT_CHK : integer := 0;
      SRC_INPUT_REG : integer := 1
    );
    port (
      src_clk : in std_logic;
      src_in : in std_logic;
      dest_clk : in std_logic;
      dest_out : out std_logic
    );
  end component;

  component xpm_cdc_array_single
    generic (
      DEST_SYNC_FF : integer := 4;
      INIT_SYNC_FF : integer := 0;
      SIM_ASSERT_CHK : integer := 0;
      SRC_INPUT_REG : integer := 1;
      WIDTH : integer := 2
    );
    port (
      src_clk : in std_logic;
      src_in : in std_logic_vector(WIDTH - 1 downto 0);
      dest_clk : in std_logic;
      dest_out : out std_logic_vector(WIDTH - 1 downto 0)
    );
  end component;

  component xpm_cdc_gray
    generic (
      DEST_SYNC_FF : integer := 4;
      INIT_SYNC_FF : integer := 0;
      SIM_ASSERT_CHK : integer := 0;
      REG_OUTPUT : integer := 0;
      SIM_LOSSLESS_GRAY_CHK : integer := 0;
      WIDTH : integer := 2
    );
    port (
      src_clk : in std_logic;
      src_in_bin : in std_logic_vector(WIDTH - 1 downto 0);
      dest_clk : in std_logic;
      dest_out_bin : out std_logic_vector(WIDTH - 1 downto 0)
    );
  end component;

  component xpm_cdc_handshake
    generic (
      DEST_EXT_HSK : integer := 1;
      DEST_SYNC_FF : integer := 4;
      INIT_SYNC_FF : integer := 0;
      SIM_ASSERT_CHK : integer := 0;
      SRC_SYNC_FF : integer := 4;
      WIDTH : integer := 1
    );
    port (
      src_clk : in std_logic;
      src_in : in std_logic_vector(WIDTH - 1 downto 0);
      src_send : in std_logic;
      src_rcv : out std_logic;
      dest_clk : in std_logic;
      dest_req : out std_logic;
      dest_ack : in std_logic;
      dest_out : out std_logic_vector(WIDTH - 1 downto 0)
    );
  end component;

  component xpm_cdc_pulse
    generic (
      DEST_SYNC_FF : integer := 4;
      INIT_SYNC_FF : integer := 0;
      SIM_ASSERT_CHK : integer := 0;
      REG_OUTPUT : integer := 0;
      RST_USED : integer := 1
    );
    port (
      src_clk : in std_logic;
      src_pulse : in std_logic;
      src_rst : in std_logic;
      dest_clk : in std_logic;
      dest_rst : in std_logic;
      dest_pulse : out std_logic
    );
  end component;

  component xpm_cdc_async_rst
    generic (
      DEST_SYNC_FF : integer := 4;
      INIT_SYNC_FF : integer := 0;
      RST_ACTIVE_HIGH : integer := 0
    );
    port (
      src_arst : in std_logic;
      dest_clk : in std_logic;
      dest_arst : out std_logic
    );
  end component;

  component xpm_cdc_sync_rst
    generic (
      DEST_SYNC_FF : integer := 4;
      INIT : integer := 1;
      INIT_SYNC_FF : integer := 0;
      SIM_ASSERT_CHK : integer := 0
    );
    port (
      src_rst : in std_logic;
      dest_clk : in std_logic;
      dest_rst : out std_logic
    );
  end component;

  component xpm_fifo_sync
    generic (
      CASCADE_HEIGHT : integer := 0;
      DOUT_RESET_VALUE : string := "0";
      ECC_MODE : string := "no_ecc";
      FIFO_MEMORY_TYPE : string := "auto";
      FIFO_READ_LATENCY : integer := 1;
      FIFO_WRITE_DEPTH : integer := 2048;
      FULL_RESET_VALUE : integer := 0;
      PROG_EMPTY_THRESH : integer := 10;
      PROG_FULL_THRESH : integer := 10;
      RD_DATA_COUNT_WIDTH : integer := 1;
      READ_DATA_WIDTH : integer := 32;
      READ_MODE : string := "std";
      SIM_ASSERT_CHK : integer := 0;
      USE_ADV_FEATURES : string := "0707";
      WAKEUP_TIME : integer := 0;
      WRITE_DATA_WIDTH : integer := 32;
      WR_DATA_COUNT_WIDTH : integer := 1
    );
    port (
      almost_empty : out std_logic;
      almost_full : out std_logic;
      data_valid : out std_logic;
      dbiterr : out std_logic;
      dout : out std_logic_vector(READ_DATA_WIDTH - 1 downto 0);
      empty : out std_logic;
      full : out std_logic;
      overflow : out std_logic;
      prog_empty : out std_logic;
      prog_full : out std_logic;
      rd_data_count : out std_logic_vector(RD_DATA_COUNT_WIDTH - 1 downto 0);
      rd_rst_busy : out std_logic;
      sbiterr : out std_logic;
      underflow : out std_logic;
      wr_ack : out std_logic;
      wr_data_count : out std_logic_vector(WR_DATA_COUNT_WIDTH - 1 downto 0);
      wr_rst_busy : out std_logic;
      din : in std_logic_vector(WRITE_DATA_WIDTH - 1 downto 0);
      injectdbiterr : in std_logic;
      injectsbiterr : in std_logic;
      rd_en : in std_logic;
      rst : in std_logic;
      sleep : in std_logic;
      wr_clk : in std_logic;
      wr_en : in std_logic
    );
  end component;

  component xpm_fifo_async
    generic (
      CASCADE_HEIGHT : integer := 0;
      CDC_SYNC_STAGES : integer := 2;
      DOUT_RESET_VALUE : string := "0";
      ECC_MODE : string := "no_ecc";
      FIFO_MEMORY_TYPE : string := "auto";
      FIFO_READ_LATENCY : integer := 1;
      FIFO_WRITE_DEPTH : integer := 2048;
      FULL_RESET_VALUE : integer := 0;
      PROG_EMPTY_THRESH : integer := 10;
      PROG_FULL_THRESH : integer := 10;
      RD_DATA_COUNT_WIDTH : integer := 1;
      READ_DATA_WIDTH : integer := 32;
      READ_MODE : string := "std";
      RELATED_CLOCKS : integer := 0;
      SIM_ASSERT_CHK : integer := 0;
      USE_ADV_FEATURES : string := "0707";
      WAKEUP_TIME : integer := 0;
      WRITE_DATA_WIDTH : integer := 32;
      WR_DATA_COUNT_WIDTH : integer := 1
    );
    port (
      almost_empty : out std_logic;
      almost_full : out std_logic;
      data_valid : out std_logic;
      dbiterr : out std_logic;
      dout : out std_logic_vector(READ_DATA_WIDTH - 1 downto 0);
      empty : out std_logic;
      full : out std_logic;
      overflow : out std_logic;
      prog_empty : out std_logic;
      prog_full : out std_logic;
      rd_data_count : out std_logic_vector(RD_DATA_COUNT_WIDTH - 1 downto 0);
      rd_rst_busy : out std_logic;
      sbiterr : out std_logic;
      underflow : out std_logic;
      wr_ack : out std_logic;
      wr_data_count : out std_logic_vector(WR_DATA_COUNT_WIDTH - 1 downto 0);
      wr_rst_busy : out std_logic;
      din : in std_logic_vector(WRITE_DATA_WIDTH - 1 downto 0);
      injectdbiterr : in std_logic;
      injectsbiterr : in std_logic;
      rd_clk : in std_logic;
      rd_en : in std_logic;
      rst : in std_logic;
      sleep : in std_logic;
      wr_clk : in std_logic;
      wr_en : in std_logic
    );
  end component;

  component xpm_memory_spram
    generic (
      ADDR_WIDTH_A : integer := 6;
      AUTO_SLEEP_TIME : integer := 0;
      BYTE_WRITE_WIDTH_A : integer := 32;
      CASCADE_HEIGHT : integer := 0;
      ECC_MODE : string := "no_ecc";
      MEMORY_INIT_FILE : string := "none";
      MEMORY_INIT_PARAM : string := "0";
      MEMORY_OPTIMIZATION : string := "true";
      MEMORY_PRIMITIVE : string := "auto";
      MEMORY_SIZE : integer := 2048;
      MESSAGE_CONTROL : integer := 0;
      READ_DATA_WIDTH_A : integer := 32;
      READ_LATENCY_A : integer := 2;
      READ_RESET_VALUE_A : string := "0";
      RST_MODE_A : string := "SYNC";
      SIM_ASSERT_CHK : integer := 0;
      USE_MEM_INIT : integer := 1;
      WAKEUP_TIME : string := "disable_sleep";
      WRITE_DATA_WIDTH_A : integer := 32;
      WRITE_MODE_A : string := "read_first"
    );
    port (
      dbiterra : out std_logic;
      douta : out std_logic_vector(READ_DATA_WIDTH_A - 1 downto 0);
      sbiterra : out std_logic;
      addra : in std_logic_vector(ADDR_WIDTH_A - 1 downto 0);
      clka : in std_logic;
      dina : in std_logic_vector(WRITE_DATA_WIDTH_A - 1 downto 0);
      ena : in std_logic;
      injectdbiterra : in std_logic;
      injectsbiterra : in std_logic;
      regcea : in std_logic;
      rsta : in std_logic;
      wea : in std_logic_vector(WRITE_DATA_WIDTH_A / BYTE_WRITE_WIDTH_A - 1 downto 0);
      sleep : in std_logic
    );
  end component;

  component xpm_memory_sprom
    generic (
      ADDR_WIDTH_A : integer := 6;
      AUTO_SLEEP_TIME : integer := 0;
      CASCADE_HEIGHT : integer := 0;
      ECC_MODE : string := "no_ecc";
      MEMORY_INIT_FILE : string := "none";
      MEMORY_INIT_PARAM : string := "0";
      MEMORY_OPTIMIZATION : string := "true";
      MEMORY_PRIMITIVE : string := "auto";
      MEMORY_SIZE : integer := 2048;
      MESSAGE_CONTROL : integer := 0;
      READ_DATA_WIDTH_A : integer := 32;
      READ_LATENCY_A : integer := 2;
      READ_RESET_VALUE_A : string := "0";
      RST_MODE_A : string := "SYNC";
      SIM_ASSERT_CHK : integer := 0;
      USE_MEM_INIT : integer := 1;
      WAKEUP_TIME : string := "disable_sleep"
    );
    port (
      dbiterra : out std_logic;
      douta : out std_logic_vector(READ_DATA_WIDTH_A - 1 downto 0);
      sbiterra : out std_logic;
      addra : in std_logic_vector(ADDR_WIDTH_A - 1 downto 0);
      clka : in std_logic;
      ena : in std_logic;
      regcea : in std_logic;
      rsta : in std_logic;
      sleep : in std_logic
    );
  end component;

  component xpm_memory_dprom
    generic (
      ADDR_WIDTH_A : integer := 6;
      ADDR_WIDTH_B : integer := 6;
      AUTO_SLEEP_TIME : integer := 0;
      CASCADE_HEIGHT : integer := 0;
      CLOCKING_MODE : string := "common_clock";
      ECC_MODE : string := "no_ecc";
      MEMORY_INIT_FILE : string := "none";
      MEMORY_INIT_PARAM : string := "0";
      MEMORY_OPTIMIZATION : string := "true";
      MEMORY_PRIMITIVE : string := "auto";
      MEMORY_SIZE : integer := 2048;
      MESSAGE_CONTROL : integer := 0;
      READ_DATA_WIDTH_A : integer := 32;
      READ_DATA_WIDTH_B : integer := 32;
      READ_LATENCY_A : integer := 2;
      READ_LATENCY_B : integer := 2;
      READ_RESET_VALUE_A : string := "0";
      READ_RESET_VALUE_B : string := "0";
      RST_MODE_A : string := "SYNC";
      RST_MODE_B : string := "SYNC";
      SIM_ASSERT_CHK : integer := 0;
      USE_MEM_INIT : integer := 1;
      WAKEUP_TIME : string := "disable_sleep"
    );
    port (
      dbiterra : out std_logic;
      douta : out std_logic_vector(READ_DATA_WIDTH_A - 1 downto 0);
      sbiterra : out std_logic;
      dbiterrb : out std_logic;
      doutb : out std_logic_vector(READ_DATA_WIDTH_B - 1 downto 0);
      sbiterrb : out std_logic;
      addra : in std_logic_vector(ADDR_WIDTH_A - 1 downto 0);
      clka : in std_logic;
      ena : in std_logic;
      regcea : in std_logic;
      rsta : in std_logic;
      addrb : in std_logic_vector(ADDR_WIDTH_B - 1 downto 0);
      clkb : in std_logic;
      enb : in std_logic;
      regceb : in std_logic;
      rstb : in std_logic;
      sleep : in std_logic
    );
  end component;

  component xpm_memory_sdpram
    generic (
      ADDR_WIDTH_A : integer := 6;
      ADDR_WIDTH_B : integer := 6;
      AUTO_SLEEP_TIME : integer := 0;
      BYTE_WRITE_WIDTH_A : integer := 32;
      CASCADE_HEIGHT : integer := 0;
      CLOCKING_MODE : string := "common_clock";
      ECC_MODE : string := "no_ecc";
      MEMORY_INIT_FILE : string := "none";
      MEMORY_INIT_PARAM : string := "0";
      MEMORY_OPTIMIZATION : string := "true";
      MEMORY_PRIMITIVE : string := "auto";
      MEMORY_SIZE : integer := 2048;
      MESSAGE_CONTROL : integer := 0;
      READ_DATA_WIDTH_B : integer := 32;
      READ_LATENCY_B : integer := 2;
      READ_RESET_VALUE_B : string := "0";
      RST_MODE_B : string := "SYNC";
      SIM_ASSERT_CHK : integer := 0;
      USE_MEM_INIT : integer := 1;
      WAKEUP_TIME : string := "disable_sleep";
      WRITE_DATA_WIDTH_A : integer := 32;
      WRITE_MODE_A : string := "read_first"
    );
    port (
      dbiterrb : out std_logic;
      doutb : out std_logic_vector(READ_DATA_WIDTH_B - 1 downto 0);
      sbiterrb : out std_logic;
      addra : in std_logic_vector(ADDR_WIDTH_A - 1 downto 0);
      clka : in std_logic;
      dina : in std_logic_vector(WRITE_DATA_WIDTH_A - 1 downto 0);
      ena : in std_logic;
      wea : in std_logic_vector(WRITE_DATA_WIDTH_A / BYTE_WRITE_WIDTH_A - 1 downto 0);
      addrb : in std_logic_vector(ADDR_WIDTH_B - 1 downto 0);
      clkb : in std_logic;
      enb : in std_logic;
      regceb : in std_logic;
      rstb : in std_logic;
      injectdbiterra : in std_logic;
      injectsbiterra : in std_logic;
      sleep : in std_logic
    );
  end component;

  component xpm_memory_tdpram
    generic (
      ADDR_WIDTH_A : integer := 6;
      ADDR_WIDTH_B : integer := 6;
      AUTO_SLEEP_TIME : integer := 0;
      BYTE_WRITE_WIDTH_A : integer := 32;
      BYTE_WRITE_WIDTH_B : integer := 32;
      CASCADE_HEIGHT : integer := 0;
      CLOCKING_MODE : string := "common_clock";
      ECC_MODE : string := "no_ecc";
      MEMORY_INIT_FILE : string := "none";
      MEMORY_INIT_PARAM : string := "0";
      MEMORY_OPTIMIZATION : string := "true";
      MEMORY_PRIMITIVE : string := "auto";
      MEMORY_SIZE : integer := 2048;
      MESSAGE_CONTROL : integer := 0;
      READ_DATA_WIDTH_A : integer := 32;
      READ_DATA_WIDTH_B : integer := 32;
      READ_LATENCY_A : integer := 2;
      READ_LATENCY_B : integer := 2;
      READ_RESET_VALUE_A : string := "0";
      READ_RESET_VALUE_B : string := "0";
      RST_MODE_A : string := "SYNC";
      RST_MODE_B : string := "SYNC";
      SIM_ASSERT_CHK : integer := 0;
      USE_MEM_INIT : integer := 1;
      WAKEUP_TIME : string := "disable_sleep";
      WRITE_DATA_WIDTH_A : integer := 32;
      WRITE_DATA_WIDTH_B : integer := 32;
      WRITE_MODE_A : string := "read_first";
      WRITE_MODE_B : string := "read_first"
    );
    port (
      dbiterra : out std_logic;
      douta : out std_logic_vector(READ_DATA_WIDTH_A - 1 downto 0);
      sbiterra : out std_logic;
      dbiterrb : out std_logic;
      doutb : out std_logic_vector(READ_DATA_WIDTH_B - 1 downto 0);
      sbiterrb : out std_logic;
      addra : in std_logic_vector(ADDR_WIDTH_A - 1 downto 0);
      clka : in std_logic;
      dina : in std_logic_vector(WRITE_DATA_WIDTH_A - 1 downto 0);
      ena : in std_logic;
      injectdbiterra : in std_logic;
      injectsbiterra : in std_logic;
      regcea : in std_logic;
      rsta : in std_logic;
      wea : in std_logic_vector(WRITE_DATA_WIDTH_A / BYTE_WRITE_WIDTH_A - 1 downto 0);
      addrb : in std_logic_vector(ADDR_WIDTH_B - 1 downto 0);
      clkb : in std_logic;
      dinb : in std_logic_vector(WRITE_DATA_WIDTH_B - 1 downto 0);
      enb : in std_logic;
      injectdbiterrb : in std_logic;
      injectsbiterrb : in std_logic;
      regceb : in std_logic;
      rstb : in std_logic;
      web : in std_logic_vector(WRITE_DATA_WIDTH_B / BYTE_WRITE_WIDTH_B - 1 downto 0);
      sleep : in std_logic
    );
  end component;

end package vcomponents;
