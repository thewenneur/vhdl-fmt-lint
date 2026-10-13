-- Declarations of package STD.ENV (IEEE 1076-2019, 16.5), written for
-- vhone's name resolution.
package env is
  procedure stop (status : integer);
  procedure stop;
  procedure finish (status : integer);
  procedure finish;
  function resolution_limit return delay_length;

  type dayofweek is (sunday, monday, tuesday, wednesday, thursday, friday, saturday);
  type time_record is record
    microsecond : integer range 0 to 999999;
    second : integer range 0 to 60;
    minute : integer range 0 to 59;
    hour : integer range 0 to 23;
    day : integer range 1 to 31;
    month : integer range 0 to 11;
    year : integer range 1 to 4095;
    weekday : dayofweek;
    dayofyear : integer range 0 to 365;
  end record time_record;
  impure function localtime return time_record;
  impure function gmtime return time_record;
  impure function epoch return real;
  function localtime (timer : real) return time_record;
  function gmtime (timer : real) return time_record;
  function epoch (trec : time_record) return real;
  function time_to_seconds (time_val : in time) return real;
  function seconds_to_time (real_val : in real) return time;
  function to_string (trec : time_record; frac_digits : integer range 0 to 6 := 0) return string;

  impure function getenv (name : string) return string;
  function vhdl_version return string;
  function tool_type return string;
  function tool_vendor return string;
  function tool_name return string;
  function tool_edition return string;
  function tool_version return string;

  type directory_items is access line_vector;
  type directory is record
    name : line;
    items : directory_items;
  end record;
  type dir_open_status is (status_ok, status_not_found, status_no_directory, status_access_denied, status_error);
  type dir_create_status is (status_ok, status_item_exists, status_access_denied, status_error);
  type dir_delete_status is (status_ok, status_no_directory, status_not_empty, status_access_denied, status_error);
  impure function dir_open (dir : out directory; path : in string) return dir_open_status;
  procedure dir_open (dir : out directory; path : in string; status : out dir_open_status);
  procedure dir_close (variable dir : inout directory);
  impure function dir_itemexists (path : in string) return boolean;
  impure function dir_itemisdir (path : in string) return boolean;
  impure function dir_itemisfile (path : in string) return boolean;
  procedure dir_workingdir (path : in string; status : out dir_open_status);
  impure function dir_workingdir (path : in string) return dir_open_status;
  impure function dir_workingdir return string;
  impure function dir_createdir (path : in string; parents : boolean := false) return dir_create_status;
  procedure dir_createdir (path : in string; status : out dir_create_status);
  procedure dir_createdir (path : in string; parents : boolean; status : out dir_create_status);
  impure function dir_deletedir (path : in string; recursive : boolean := false) return dir_delete_status;
  procedure dir_deletedir (path : in string; status : out dir_delete_status);
  procedure dir_deletedir (path : in string; recursive : boolean; status : out dir_delete_status);
  impure function dir_deletefile (path : in string) return file_open_status;
  procedure dir_deletefile (path : in string; status : out file_open_status);
  constant dir_separator : string := "/";

  impure function file_name return line;
  impure function file_name return string;
  impure function file_path return line;
  impure function file_path return string;
  impure function file_line return positive;
  impure function file_line return string;

  type call_path_element is record
    name : line;
    file_name : line;
    file_path : line;
    file_line : positive;
  end record;
  type call_path_vector is array (natural range <>) of call_path_element;
  type call_path_vector_ptr is access call_path_vector;
  impure function get_call_path return call_path_vector_ptr;
  function to_string (variable call_path : call_path_vector; separator : string := "" & lf) return string;
  function to_string (variable call_path : call_path_vector_ptr; separator : string := "" & lf) return string;
  function to_string (variable call_path_element : call_path_element) return string;

  impure function psl_assert_failed return boolean;
  impure function psl_is_covered return boolean;
  procedure setpslcoverassert (enable : boolean := true);
  impure function getpslcoverassert return boolean;
  impure function psl_is_assert_covered return boolean;
  procedure clearpslstate;
  impure function isvhdlassertfailed return boolean;
  impure function isvhdlassertfailed (level : severity_level) return boolean;
  impure function getvhdlassertcount return natural;
  impure function getvhdlassertcount (level : severity_level) return natural;
  procedure clearvhdlassert;
  procedure setvhdlassertenable (enable : boolean := true);
  procedure setvhdlassertenable (level : severity_level := note; enable : boolean := true);
  impure function getvhdlassertenable (level : severity_level := note) return boolean;
  procedure setvhdlassertformat (level : severity_level; format : string);
  procedure setvhdlassertformat (level : severity_level; format : string; valid : out boolean);
  impure function getvhdlassertformat (level : severity_level) return string;
  procedure setvhdlreadseverity (level : severity_level := failure);
  impure function getvhdlreadseverity return severity_level;
end package env;
