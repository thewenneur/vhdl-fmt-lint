-- Declarations of package STD.STANDARD (IEEE 1076-2019, 16.3), written for
-- vhone's name resolution. Operations the standard declares
-- implicitly for every type are listed once, as plain declarations, so that
-- they resolve by name.
package standard is
  type boolean is (false, true);
  type bit is ('0', '1');
  type character is (
    nul, soh, stx, etx, eot, enq, ack, bel, bs, ht, lf, vt, ff, cr, so, si,
    dle, dc1, dc2, dc3, dc4, nak, syn, etb, can, em, sub, esc, fsp, gsp, rsp, usp,
    ' ', '!', '"', '#', '$', '%', '&', ''', '(', ')', '*', '+', ',', '-', '.', '/',
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', ':', ';', '<', '=', '>', '?',
    '@', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O',
    'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '[', '\', ']', '^', '_',
    '`', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o',
    'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '{', '|', '}', '~', del,
    c128, c129, c130, c131, c132, c133, c134, c135, c136, c137, c138, c139, c140, c141, c142, c143,
    c144, c145, c146, c147, c148, c149, c150, c151, c152, c153, c154, c155, c156, c157, c158, c159
  );
  type severity_level is (note, warning, error, failure);
  type integer is range -9223372036854775807 - 1 to 9223372036854775807;
  type real is range -1.7976931348623157e308 to 1.7976931348623157e308;
  type time is range -9223372036854775807 - 1 to 9223372036854775807
    units
      fs;
      ps = 1000 fs;
      ns = 1000 ps;
      us = 1000 ns;
      ms = 1000 us;
      sec = 1000 ms;
      min = 60 sec;
      hr = 60 min;
    end units;
  subtype delay_length is time range 0 fs to time'high;
  impure function now return delay_length;
  subtype natural is integer range 0 to integer'high;
  subtype positive is integer range 1 to integer'high;
  type string is array (positive range <>) of character;
  type boolean_vector is array (natural range <>) of boolean;
  type bit_vector is array (natural range <>) of bit;
  type integer_vector is array (natural range <>) of integer;
  type real_vector is array (natural range <>) of real;
  type time_vector is array (natural range <>) of time;
  type file_open_kind is (read_mode, write_mode, append_mode, read_write_mode);
  type file_open_status is (open_ok, status_error, name_error, mode_error);
  type file_origin_kind is (file_origin_begin, file_origin_current, file_origin_end);
  type file_open_state is (state_open, state_closed);
  attribute foreign : string;

  -- Operations declared implicitly for each type.
  function rising_edge (signal s : bit) return boolean;
  function falling_edge (signal s : bit) return boolean;
  function minimum (l, r : integer) return integer;
  function maximum (l, r : integer) return integer;
  function to_string (value : integer) return string;
  function to_bstring (value : bit_vector) return string;
  function to_binary_string (value : bit_vector) return string;
  function to_ostring (value : bit_vector) return string;
  function to_octal_string (value : bit_vector) return string;
  function to_hstring (value : bit_vector) return string;
  function to_hex_string (value : bit_vector) return string;
  procedure deallocate (p : inout integer);
  procedure file_open (f : integer; external_name : string; open_kind : file_open_kind := read_mode);
  procedure file_close (f : integer);
  procedure file_rewind (f : integer);
  procedure file_seek (f : integer; offset : integer; origin : file_origin_kind := file_origin_begin);
  procedure file_truncate (f : integer; size : integer; origin : file_origin_kind := file_origin_begin);
  function file_mode (f : integer) return file_open_kind;
  function file_position (f : integer; origin : file_origin_kind := file_origin_begin) return integer;
  function file_size (f : integer) return integer;
  function file_canseek (f : integer) return boolean;
  function file_state (f : integer) return file_open_state;
  procedure read (f : integer; value : out integer);
  procedure write (f : integer; value : integer);
  procedure flush (f : integer);
  function endfile (f : integer) return boolean;
end package standard;
