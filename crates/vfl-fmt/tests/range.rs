//! Range formatting: only the selected lines change, alignment stays
//! consistent, and formatting the same lines again changes nothing.

use std::ops::Range;
use std::path::Path;

use vfl_config::FormatConfig;
use vfl_fmt::{Edit, apply_edits, format, format_range, format_ranges_in, format_typed};
use vfl_syntax::Standard;

fn edits(text: &str, range: Range<usize>) -> Vec<Edit> {
    format_range(text, range, &FormatConfig::default(), Standard::Vhdl2019)
        .unwrap_or_else(|e| panic!("{e}"))
}

/// Formats `range` of `text`, and checks that formatting what changed
/// again changes nothing.
fn fmt_range(text: &str, range: Range<usize>) -> String {
    let first = edits(text, range);
    let out = apply_edits(text, &first);
    // Where the edits landed in the new text.
    let mut shift: isize = 0;
    let mut landed = Vec::new();
    for edit in &first {
        let start = (edit.range.start as isize + shift) as usize;
        landed.push(start..start + edit.text.len());
        shift += edit.text.len() as isize - edit.range.len() as isize;
    }
    for range in landed {
        let again = edits(&out, range);
        assert!(again.is_empty(), "not idempotent: {again:?}\n{out}");
    }
    out
}

/// The byte range of 1-based line `n`.
fn line(text: &str, n: usize) -> Range<usize> {
    let start: usize = text.split_inclusive('\n').take(n - 1).map(str::len).sum();
    let len = text[start..].find('\n').unwrap_or(text.len() - start);
    start..start + len
}

#[test]
fn only_the_selected_statement_changes() {
    let text = "architecture a of e is\nbegin\nx<=a;\n\n    y<=b and c;\n\nz  <=  d;\nend;\n";
    assert_eq!(
        fmt_range(text, line(text, 5)),
        "architecture a of e is\nbegin\nx<=a;\n\n  y <= b and c;\n\nz  <=  d;\nend;\n"
    );
}

#[test]
fn a_cursor_formats_its_line() {
    let text = "architecture a of e is\nbegin\nx<=a;\n\n    y<=b and c;\nend;\n";
    let at = line(text, 5).start + 6;
    assert_eq!(
        fmt_range(text, at..at),
        "architecture a of e is\nbegin\nx<=a;\n\n  y <= b and c;\nend;\n"
    );
}

#[test]
fn whole_line_selection_stops_before_the_next_line() {
    let text = "architecture a of e is\nbegin\nx<=a;\n\ny<=b;\nend;\n";
    let r = line(text, 3);
    assert_eq!(
        fmt_range(text, r.start..r.end + 1),
        "architecture a of e is\nbegin\n  x <= a;\n\ny<=b;\nend;\n"
    );
}

#[test]
fn aligned_assignments_are_formatted_together() {
    let text = "architecture a of e is\nbegin\nx<=a;\n    long_name<=b;\nend;\n";
    assert_eq!(
        fmt_range(text, line(text, 3)),
        "architecture a of e is\nbegin\n  x         <= a;\n  long_name <= b;\nend;\n"
    );
}

#[test]
fn alignment_runs_are_formatted_whole() {
    let text = "entity e is\n  port (\n    clk : in bit;\n    data_valid : in bit;\n    q : out bit\n  );\nend;\n";
    assert_eq!(
        fmt_range(text, line(text, 4)),
        "entity e is\n  port (\n    clk        : in  bit;\n    data_valid : in  bit;\n    q          : out bit\n  );\nend;\n"
    );
}

#[test]
fn a_statement_split_over_lines_is_formatted_whole() {
    let text = "architecture a of e is\nbegin\nx<=a when s='1'\nelse\nb;\n\ny<=c;\nend;\n";
    let out = fmt_range(text, line(text, 4));
    assert_eq!(
        out,
        "architecture a of e is\nbegin\n  x <= a when s = '1'\n       else\n       b;\n\ny<=c;\nend;\n"
    );
}

#[test]
fn a_construct_keyword_does_not_format_the_construct() {
    let text =
        "architecture a of e is\nbegin\n process   (clk)  begin\nx<=a;\nend process;\nend;\n";
    let out = fmt_range(text, line(text, 3));
    assert_eq!(
        out,
        "architecture a of e is\nbegin\n  process (clk)\n  begin\nx<=a;\nend process;\nend;\n"
    );
}

#[test]
fn files_with_syntax_errors_are_not_formatted() {
    let text = "entity e is port (a : in bit) end;";
    assert!(format_range(text, 0..5, &FormatConfig::default(), Standard::Vhdl2008).is_err());
}

#[test]
fn protected_lines_are_left_alone() {
    let text = "architecture a of e is\nbegin\n-- vfl: off\nx<=a;\n-- vfl: on\ny<=b;\nend;\n";
    let all = 0..text.len();
    assert_eq!(
        fmt_range(text, all),
        "architecture a of e is\nbegin\n  -- vfl: off\nx<=a;\n  -- vfl: on\n  y <= b;\nend;\n"
    );
    assert!(edits(text, line(text, 4)).is_empty());
}

#[test]
fn crlf_and_byte_order_mark_are_kept() {
    let text = "\u{feff}architecture a of e is\r\nbegin\r\nx<=a;\r\n\r\ny<=b;\r\nend;\r\n";
    let r = line(text, 3);
    assert_eq!(
        fmt_range(text, r.start + 1..r.start + 2),
        "\u{feff}architecture a of e is\r\nbegin\r\n  x <= a;\r\n\r\ny<=b;\r\nend;\r\n"
    );
}

#[test]
fn formatting_every_line_matches_formatting_the_file() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "vhd") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let config = FormatConfig::default();
        let whole = format(&text, &config, Standard::Vhdl2019).unwrap();
        let ranged = apply_edits(
            &text,
            &format_range(&text, 0..text.len(), &config, Standard::Vhdl2019).unwrap(),
        );
        // Blank lines at the start and end of the file are outside every line.
        assert_eq!(ranged.trim_end(), whole.trim_end(), "{}", path.display());
    }
}

#[test]
fn each_line_alone_is_idempotent() {
    let dirs = ["tests/data", "../vfl-syntax/tests/data/ok"];
    for dir in dirs {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            for n in 1..=text.lines().count() {
                // Laying out only the part around the line changes nothing.
                let config = FormatConfig::default();
                let r = line(&text, n);
                assert_eq!(
                    edits(&text, r.clone()),
                    format_ranges_in(&text, &[r], &config, Standard::Vhdl2019, false).unwrap(),
                    "{} line {n}",
                    path.display()
                );
                let out = fmt_range(&text, line(&text, n));
                // The result still formats to the same file.
                assert_eq!(
                    format(&out, &FormatConfig::default(), Standard::Vhdl2019).unwrap(),
                    format(&text, &FormatConfig::default(), Standard::Vhdl2019).unwrap(),
                    "{} line {n}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn format_on_type_follows_the_config() {
    let text = "architecture a of e is\nbegin\nx<=a;\nend;\n";
    let cursor = text.find("a;").unwrap() + 2;
    let mut config = FormatConfig::default();
    let typed = |config: &FormatConfig, ch| {
        apply_edits(
            text,
            &format_typed(text, cursor, ch, config, Standard::Vhdl2019).unwrap(),
        )
    };
    assert_eq!(
        typed(&config, ';'),
        "architecture a of e is\nbegin\n  x <= a;\nend;\n"
    );
    assert_eq!(typed(&config, 'x'), text);
    config.on_type = false;
    assert_eq!(typed(&config, ';'), text);
}
