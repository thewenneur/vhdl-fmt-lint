-- Declarations of commonly used Intel (Altera) altera_mf megafunctions,
-- written for vhdl-fmt-lint's name resolution from Intel's public user
-- guides. Ports and the parameters most often set; what is not declared
-- here is not checked. Not for simulation or synthesis: those use the
-- vendor's own library.
library ieee;
use ieee.std_logic_1164.all;

package altera_mf_components is

  component altsyncram
    generic (
      address_aclr_a : string := "NONE";
      address_aclr_b : string := "NONE";
      address_reg_b : string := "CLOCK1";
      byte_size : natural := 8;
      clock_enable_input_a : string := "NORMAL";
      clock_enable_input_b : string := "NORMAL";
      clock_enable_output_a : string := "NORMAL";
      clock_enable_output_b : string := "NORMAL";
      init_file : string := "UNUSED";
      intended_device_family : string := "unused";
      lpm_hint : string := "UNUSED";
      lpm_type : string := "altsyncram";
      numwords_a : natural := 0;
      numwords_b : natural := 0;
      operation_mode : string := "BIDIR_DUAL_PORT";
      outdata_aclr_a : string := "NONE";
      outdata_aclr_b : string := "NONE";
      outdata_reg_a : string := "UNREGISTERED";
      outdata_reg_b : string := "UNREGISTERED";
      power_up_uninitialized : string := "FALSE";
      ram_block_type : string := "AUTO";
      read_during_write_mode_mixed_ports : string := "DONT_CARE";
      read_during_write_mode_port_a : string := "NEW_DATA_NO_NBE_READ";
      read_during_write_mode_port_b : string := "NEW_DATA_NO_NBE_READ";
      width_a : natural := 1;
      width_b : natural := 1;
      width_byteena_a : natural := 1;
      width_byteena_b : natural := 1;
      widthad_a : natural := 1;
      widthad_b : natural := 1;
      wrcontrol_wraddress_reg_b : string := "CLOCK1"
    );
    port (
      aclr0 : in std_logic := '0';
      aclr1 : in std_logic := '0';
      address_a : in std_logic_vector(widthad_a - 1 downto 0);
      address_b : in std_logic_vector(widthad_b - 1 downto 0) := (others => '1');
      addressstall_a : in std_logic := '0';
      addressstall_b : in std_logic := '0';
      byteena_a : in std_logic_vector(width_byteena_a - 1 downto 0) := (others => '1');
      byteena_b : in std_logic_vector(width_byteena_b - 1 downto 0) := (others => '1');
      clock0 : in std_logic := '1';
      clock1 : in std_logic := '1';
      clocken0 : in std_logic := '1';
      clocken1 : in std_logic := '1';
      clocken2 : in std_logic := '1';
      clocken3 : in std_logic := '1';
      data_a : in std_logic_vector(width_a - 1 downto 0) := (others => '1');
      data_b : in std_logic_vector(width_b - 1 downto 0) := (others => '1');
      eccstatus : out std_logic_vector(2 downto 0);
      q_a : out std_logic_vector(width_a - 1 downto 0);
      q_b : out std_logic_vector(width_b - 1 downto 0);
      rden_a : in std_logic := '1';
      rden_b : in std_logic := '1';
      wren_a : in std_logic := '0';
      wren_b : in std_logic := '0'
    );
  end component;

  component scfifo
    generic (
      add_ram_output_register : string := "OFF";
      allow_rwcycle_when_full : string := "OFF";
      almost_empty_value : natural := 0;
      almost_full_value : natural := 0;
      intended_device_family : string := "unused";
      lpm_hint : string := "UNUSED";
      lpm_numwords : natural := 2;
      lpm_showahead : string := "OFF";
      lpm_type : string := "scfifo";
      lpm_width : natural := 1;
      lpm_widthu : natural := 1;
      overflow_checking : string := "ON";
      underflow_checking : string := "ON";
      use_eab : string := "ON"
    );
    port (
      aclr : in std_logic := '0';
      almost_empty : out std_logic;
      almost_full : out std_logic;
      clock : in std_logic;
      data : in std_logic_vector(lpm_width - 1 downto 0);
      empty : out std_logic;
      full : out std_logic;
      q : out std_logic_vector(lpm_width - 1 downto 0);
      rdreq : in std_logic;
      sclr : in std_logic := '0';
      usedw : out std_logic_vector(lpm_widthu - 1 downto 0);
      wrreq : in std_logic
    );
  end component;

  component dcfifo
    generic (
      add_ram_output_register : string := "OFF";
      add_usedw_msb_bit : string := "OFF";
      clocks_are_synchronized : string := "FALSE";
      delay_rdusedw : natural := 1;
      delay_wrusedw : natural := 1;
      intended_device_family : string := "unused";
      lpm_hint : string := "UNUSED";
      lpm_numwords : natural := 2;
      lpm_showahead : string := "OFF";
      lpm_type : string := "dcfifo";
      lpm_width : natural := 1;
      lpm_widthu : natural := 1;
      overflow_checking : string := "ON";
      rdsync_delaypipe : natural := 0;
      read_aclr_synch : string := "OFF";
      underflow_checking : string := "ON";
      use_eab : string := "ON";
      write_aclr_synch : string := "OFF";
      wrsync_delaypipe : natural := 0
    );
    port (
      aclr : in std_logic := '0';
      data : in std_logic_vector(lpm_width - 1 downto 0);
      q : out std_logic_vector(lpm_width - 1 downto 0);
      rdclk : in std_logic;
      rdempty : out std_logic;
      rdfull : out std_logic;
      rdreq : in std_logic;
      rdusedw : out std_logic_vector(lpm_widthu - 1 downto 0);
      wrclk : in std_logic;
      wrempty : out std_logic;
      wrfull : out std_logic;
      wrreq : in std_logic;
      wrusedw : out std_logic_vector(lpm_widthu - 1 downto 0)
    );
  end component;

  component altpll
    generic (
      bandwidth_type : string := "AUTO";
      clk0_divide_by : natural := 1;
      clk0_duty_cycle : natural := 50;
      clk0_multiply_by : natural := 1;
      clk0_phase_shift : string := "0";
      clk1_divide_by : natural := 1;
      clk1_duty_cycle : natural := 50;
      clk1_multiply_by : natural := 1;
      clk1_phase_shift : string := "0";
      clk2_divide_by : natural := 1;
      clk2_duty_cycle : natural := 50;
      clk2_multiply_by : natural := 1;
      clk2_phase_shift : string := "0";
      clk3_divide_by : natural := 1;
      clk3_duty_cycle : natural := 50;
      clk3_multiply_by : natural := 1;
      clk3_phase_shift : string := "0";
      clk4_divide_by : natural := 1;
      clk4_duty_cycle : natural := 50;
      clk4_multiply_by : natural := 1;
      clk4_phase_shift : string := "0";
      compensate_clock : string := "CLK0";
      inclk0_input_frequency : natural := 0;
      intended_device_family : string := "unused";
      lpm_hint : string := "UNUSED";
      lpm_type : string := "altpll";
      operation_mode : string := "NORMAL";
      pll_type : string := "AUTO";
      width_clock : natural := 6;
      width_phasecounterselect : natural := 4
    );
    port (
      areset : in std_logic := '0';
      clk : out std_logic_vector(width_clock - 1 downto 0);
      inclk : in std_logic_vector(1 downto 0) := (others => '0');
      locked : out std_logic;
      pfdena : in std_logic := '1';
      phasecounterselect : in std_logic_vector(width_phasecounterselect - 1 downto 0) := (others => '1');
      phasedone : out std_logic;
      phasestep : in std_logic := '1';
      phaseupdown : in std_logic := '1';
      scanclk : in std_logic := '1'
    );
  end component;

  component altddio_out
    generic (
      extend_oe_disable : string := "OFF";
      intended_device_family : string := "unused";
      invert_output : string := "OFF";
      lpm_hint : string := "UNUSED";
      lpm_type : string := "altddio_out";
      oe_reg : string := "UNUSED";
      power_up_high : string := "OFF";
      width : natural := 1
    );
    port (
      aclr : in std_logic := '0';
      aset : in std_logic := '0';
      datain_h : in std_logic_vector(width - 1 downto 0);
      datain_l : in std_logic_vector(width - 1 downto 0);
      dataout : out std_logic_vector(width - 1 downto 0);
      oe : in std_logic := '1';
      outclocken : in std_logic := '1';
      outclock : in std_logic;
      sclr : in std_logic := '0';
      sset : in std_logic := '0'
    );
  end component;

  component altddio_in
    generic (
      intended_device_family : string := "unused";
      invert_input_clocks : string := "OFF";
      lpm_hint : string := "UNUSED";
      lpm_type : string := "altddio_in";
      power_up_high : string := "OFF";
      width : natural := 1
    );
    port (
      aclr : in std_logic := '0';
      aset : in std_logic := '0';
      datain : in std_logic_vector(width - 1 downto 0);
      dataout_h : out std_logic_vector(width - 1 downto 0);
      dataout_l : out std_logic_vector(width - 1 downto 0);
      inclock : in std_logic;
      inclocken : in std_logic := '1';
      sclr : in std_logic := '0';
      sset : in std_logic := '0'
    );
  end component;

end package altera_mf_components;
