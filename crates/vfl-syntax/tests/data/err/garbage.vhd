entity e is
  port (a : in bit; $$ junk ; b : out bit);
end;

this is not vhdl;

architecture a of e is
  signal s : ;
  42 garbage here;
begin
  s <= ;
  ) stray paren;
  b <= a;
end;
