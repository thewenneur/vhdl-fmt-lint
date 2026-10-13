-- file: scopes.vhd
entity scopes is
end entity;

architecture sim of scopes is
  type rec_t is record
    field : integer;
  end record;
  type pt is protected
    procedure incr;
  end protected;
  type pt is protected body
    variable count : integer := 0;
    procedure incr is
    begin
      count := count + 1;
    end procedure;
  end protected body;
  shared variable counter : pt;
  signal r : rec_t;
  signal dup : bit;
  signal dup : bit;
  constant c : time := 10 ns;
begin
  g : for i in 0 to 3 generate
    signal local : integer;
  begin
    local <= i;
  end generate;

  main : process
    variable x : rec_t := (field => 1);
  begin
    lp : for j in 1 to 2 loop
      exit lp when j = 2;
      exit nowhere;
    end loop;
    x.field := r.field + i;
    counter.incr;
    wait for c;
    report scopes'path_name;
    wait;
  end process;

  b : block (r.field = 1) is
  begin
  end block;
end architecture;
