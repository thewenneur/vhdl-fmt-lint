//! The formatter never panics on Verilog and SystemVerilog, never fails its
//! own output check, and formatting its output again changes nothing.
//! Formatting a line and then the whole file gives what formatting the
//! whole file gives.
#![no_main]

use libfuzzer_sys::fuzz_target;
use vfl_fmt::{FormatError, apply_edits, format_ranges_verilog, format_verilog};
use vfl_verilog_syntax::{Options, Standard};

fuzz_target!(|data: &[u8]| {
    let text = vfl_fuzz::text(data);
    // Picked from the whole input, so that the text stays a plain file.
    let pick = data.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    let config = vfl_fuzz::config(pick).format;
    let defines = ["SYNTHESIS".to_owned()];
    let options = Options {
        standard: Standard::ALL[pick as usize / 4 % Standard::ALL.len()],
        defines: if pick & 1 == 1 { &defines } else { &[] },
        include: None,
    };
    let whole = match format_verilog(&text, &config, &options) {
        Ok(out) => out,
        Err(FormatError::Syntax(_)) => return,
        Err(err) => panic!("{err}"),
    };
    match format_verilog(&whole, &config, &options) {
        Ok(again) => assert_eq!(again, whole, "not idempotent"),
        Err(err) => panic!("second pass: {err}"),
    }
    // One line, picked like the configuration.
    let lines: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .filter(|&i| i < text.len())
        .collect();
    if lines.is_empty() {
        return;
    }
    let start = lines[pick as usize % lines.len()];
    let edits = match format_ranges_verilog(&text, &[start..start], &config, &options) {
        Ok(edits) => edits,
        Err(err) => panic!("range: {err}"),
    };
    let ranged = apply_edits(&text, &edits);
    match format_verilog(&ranged, &config, &options) {
        Ok(again) => assert_eq!(again, whole, "formats differently after range formatting"),
        Err(err) => panic!("after range: {err}"),
    }
});
