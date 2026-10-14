//! Helpers shared by the fuzz targets: turning fuzzer bytes into source
//! text, a standard, a configuration and a script of edits.

use vfl_config::Config;
use vfl_syntax::Standard;

/// The text of `data`: UTF-8 when it is, otherwise one char per byte, as
/// the corpus tools read Latin-1 sources.
pub fn text(data: &[u8]) -> String {
    match std::str::from_utf8(data) {
        Ok(text) => text.to_owned(),
        Err(_) => data.iter().map(|&b| b as char).collect(),
    }
}

pub fn standard(byte: u8) -> Standard {
    [Standard::Vhdl1993, Standard::Vhdl2008, Standard::Vhdl2019][byte as usize % 3]
}

/// One of a few configurations, so the fuzzers cover the options that
/// change the most code paths: the defaults, the alternate configuration
/// of `scripts/corpus.sh`, CRLF output and every lint rule.
pub fn config(byte: u8) -> Config {
    const CONFIGS: [&str; 4] = [
        "",
        r#"
[format]
keyword_case = "upper"
identifier_case = "lower"
use_tabs = true
indent = 4
end_label = "add"
end_keyword = "add"
max_width = 60
interface_list = "preserve"
association_list = "expand"
close_paren = "attached"
case_alternative = "break"
[format.align]
block_boundary = "never"
[lint]
preset = "strict"
"#,
        r#"
[format]
line_ending = "crlf"
max_width = 40
indent = 3
"#,
        r#"
[lint]
preset = "strict"
"#,
    ];
    Config::from_toml_str(CONFIGS[byte as usize % CONFIGS.len()]).expect("valid fuzz config")
}

/// Splits fuzzer input into source files and an edit script: the files
/// come first, separated by `\x01`, then a `\0` and the script. A plain
/// VHDL file is one file and no edits, so source files make good seeds.
pub fn split(data: &[u8]) -> (Vec<String>, &[u8]) {
    let (files, script) = match data.iter().position(|&b| b == 0) {
        Some(i) => (&data[..i], &data[i + 1..]),
        None => (data, &[][..]),
    };
    let files = files.split(|&b| b == 1).take(4).map(text).collect();
    (files, script)
}

/// Text edits likely to change meaning rather than just break the syntax.
#[rustfmt::skip]
const SNIPPETS: &[&str] = &[
    "\n", " ", ";", "(", ")", ",", ":", ":=", "<=", "=>", "'", "\"", "--", "/*", "*/", "end",
    "is", "begin", "signal s : bit;", "integer", "natural", "std_logic", "bit", "process",
    "if", "then", "else", "elsif", "case", "when", "others", "loop", "for", "generate",
    "entity", "architecture", "package", "body", "component", "port", "generic", "map",
    "library ieee;", "use ieee.std_logic_1164.all;", "use work.", "function", "procedure",
    "return", "type", "subtype", "record", "range", "to", "downto", "a", "b", "clk", "rst",
    "rising_edge(clk)", "x\"0F\"", "16#FF#", "'1'", "constant", "variable", "all", "wait",
    "-- vfl: off\n", "-- vfl: on\n", "-- vfl: skip\n", "\r\n", "\t", "é", "context", "view",
];

/// One edit of an edit script: replace `start..start + len` of a file
/// with `text`. Offsets are raw: callers clamp them to the file.
#[derive(Debug, Clone)]
pub struct Edit {
    pub file: usize,
    pub start: usize,
    pub len: usize,
    pub text: String,
}

/// Decodes an edit script: 5 bytes per edit (file, start as two bytes,
/// length, what to insert), with what to insert either a snippet or, for
/// the high values, that many raw bytes from the script.
pub fn edits(mut script: &[u8]) -> Vec<Edit> {
    let mut edits = Vec::new();
    while script.len() >= 5 && edits.len() < 8 {
        let [file, hi, lo, len, what] = [script[0], script[1], script[2], script[3], script[4]];
        script = &script[5..];
        let text = if (what as usize) < SNIPPETS.len() {
            SNIPPETS[what as usize].to_owned()
        } else if what >= 0xF0 {
            let n = ((what - 0xF0) as usize).min(script.len());
            let raw = text(&script[..n]);
            script = &script[n..];
            raw
        } else {
            String::new()
        };
        edits.push(Edit {
            file: file as usize,
            start: (hi as usize) << 8 | lo as usize,
            len: len as usize % 32,
            text,
        });
    }
    edits
}

/// The nearest char boundary at or before `offset` in `text`.
pub fn floor(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Applies `edit` to `text`, clamping its range to the text.
pub fn apply(text: &mut String, edit: &Edit) {
    let start = floor(text, edit.start % (text.len() + 1));
    let end = floor(text, start + edit.len).max(start);
    text.replace_range(start..end, &edit.text);
}
