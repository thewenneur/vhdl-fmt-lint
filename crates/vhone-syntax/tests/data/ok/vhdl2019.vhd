package p2019 is
  type stream_t is record
    valid, ready : std_logic;
    data : std_logic_vector(7 downto 0);
  end record;
  view master of stream_t is
    valid, data : out;
    ready : in;
  end view;
  alias slave is master'converse;
  constant c : integer := 1 when DEBUG else 0;
  function f return r of integer;
end package;

entity e2019 is
  generic (type T is private; type U is (<>));
  port (
    m : view master;
    s : view (slave) of stream_array_t;
  );
end entity;

architecture rtl of e2019 is
begin
  process
  begin
    seq : block is
      variable x : integer;
    begin
      x := 1;
    end block seq;
    return 1 when a else 2;
  end process;
end architecture;
