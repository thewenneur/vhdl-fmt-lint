-- Declarations of package STD.TEXTIO (IEEE 1076-2019, 16.4), written for
-- vhdl-fmt-lint's name resolution.
package textio is
  type line is access string;
  type line_vector is array (natural range <>) of line;
  type text is file of string;
  type side is (right, left);
  subtype width is natural;
  function justify (value : string; justified : side := right; field : width := 0) return string;
  file input : text open read_mode is "STD_INPUT";
  file output : text open write_mode is "STD_OUTPUT";
  procedure readline (file f : text; l : inout line);
  procedure read (l : inout line; value : out bit; good : out boolean);
  procedure sread (l : inout line; value : out string; strlen : out natural);
  alias string_read is sread [line, string, natural];
  procedure bread (l : inout line; value : out bit_vector; good : out boolean);
  alias binary_read is bread [line, bit_vector, boolean];
  procedure oread (l : inout line; value : out bit_vector; good : out boolean);
  alias octal_read is oread [line, bit_vector, boolean];
  procedure hread (l : inout line; value : out bit_vector; good : out boolean);
  alias hex_read is hread [line, bit_vector, boolean];
  procedure writeline (file f : text; l : inout line);
  procedure tee (file f : text; l : inout line);
  procedure write (l : inout line; value : in bit; justified : in side := right; field : in width := 0);
  procedure swrite (l : inout line; value : in string; justified : in side := right; field : in width := 0);
  alias string_write is swrite [line, string, side, width];
  procedure bwrite (l : inout line; value : in bit_vector; justified : in side := right; field : in width := 0);
  alias binary_write is bwrite [line, bit_vector, side, width];
  procedure owrite (l : inout line; value : in bit_vector; justified : in side := right; field : in width := 0);
  alias octal_write is owrite [line, bit_vector, side, width];
  procedure hwrite (l : inout line; value : in bit_vector; justified : in side := right; field : in width := 0);
  alias hex_write is hwrite [line, bit_vector, side, width];
end package textio;
