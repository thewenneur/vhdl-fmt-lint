architecture sim of tb is
begin
  stimulus : process
    variable i : integer;
    variable v : bit_vector(7 downto 0);
  begin
    wait;
    wait on a, b;
    wait until clk = '1' for 10 ns;
    wait for 5 ns;
    assert a = b;
    assert false report "boom" & integer'image(i) severity error;
    report "hello";
    report "bye" severity note;
    if a then null; elsif b then null; else null; end if;
    case v is
      when x"00" | x"01" => i := 0;
      when x"02" to x"0F" => i := 1;
      when others => null;
    end case;
    outer : for j in 0 to 7 loop
      inner : while i < 10 loop
        i := i + 1;
        next inner when i = 3;
        exit outer when i = 9;
      end loop inner;
    end loop outer;
    loop
      exit;
    end loop;
    for k in v'range loop
      v(k) := '0';
    end loop;
    for k in integer range 0 to 3 loop
      null;
    end loop;
    s <= transport '1' after 1 ns, '0' after 2 ns;
    s <= reject 1 ns inertial '1';
    s <= unaffected;
    (a, b) <= pair;
    v(3 downto 0) := "1010";
    log("message", i);
    flush;
    labelled : null;
    return;
  end process stimulus;
end architecture;
