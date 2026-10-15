library ieee;
use ieee.std_logic_1164.all;

--! Carries single-cycle pulses from one clock domain to another.
--!
--! Each source pulse flips a toggle; the destination domain synchronizes the
--! toggle through two flip-flops and pulses when it changes.
entity pulse_sync is
    port (
        clk_a   : in  std_logic; --! Source clock
        pulse_a : in  std_logic; --! Pulse in the source domain
        clk_b   : in  std_logic; --! Destination clock
        pulse_b : out std_logic  --! Pulse in the destination domain
    );
end entity pulse_sync;

architecture rtl of pulse_sync is

    signal toggle : std_logic                    := '0'             ; --! Flips on each source pulse
    signal sync   : std_logic_vector(2 downto 0) := "000"           ; --! Synchronizer and edge detector

    alias meta    : std_logic is sync(0)                            ; --! May go metastable
    alias stable  : std_logic_vector(1 downto 0) is sync(2 downto 1); --! The toggle now and one cycle before

begin

    --! Flips the toggle on each source pulse.
    source : process (clk_a) is
    begin
        if rising_edge(clk_a) then
            if pulse_a = '1' then
                toggle <= not toggle;
            end if;
        end if;
    end process source;

    --! Samples the toggle in the destination domain.
    destination : process (clk_b) is
    begin
        if rising_edge(clk_b) then
            -- `meta` is the first flip-flop of the synchronizer.
            -- vfl-ignore sema/clock-domain-crossing
            sync <= stable(0) & meta & toggle;
        end if;
    end process destination;

    pulse_b <= stable(1) xor stable(0);

end architecture rtl;
