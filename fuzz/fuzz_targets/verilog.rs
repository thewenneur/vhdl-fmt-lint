//! The Verilog and SystemVerilog parser never panics and its tree always
//! gives back the input, byte for byte, in every standard, with predefined
//! macros and included files (cycles included); module summaries never
//! panic and stay inside the text.
#![no_main]

use libfuzzer_sys::fuzz_target;
use vfl_verilog_syntax::summary::modules;
use vfl_verilog_syntax::{Options, Standard, parse_with};

fuzz_target!(|data: &[u8]| {
    // The main file, then included files separated by `\x01`, read by any
    // name ending in their index: `inc0.vh`, `inc1.vh`... The main file is
    // `inc0.vh` too, so a file can include itself.
    let files: Vec<String> = data
        .split(|&b| b == 1)
        .take(4)
        .map(vfl_fuzz::text)
        .collect();
    let text = &files[0];
    let pick = data.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    let defines: Vec<String> = ["SYNTHESIS", "WIDTH=4", "FPGA", "SIM_ONLY", "F(a)=a"]
        .iter()
        .enumerate()
        .filter(|(i, _)| pick >> i & 1 == 1)
        .map(|(_, d)| d.to_string())
        .collect();
    let include = |name: &str| -> Option<String> {
        let stem = name.trim_end_matches(".vh").trim_end_matches(".svh");
        let index: usize = stem.chars().last()?.to_digit(10)? as usize;
        files.get(index).cloned()
    };
    for standard in Standard::ALL {
        let options = Options {
            standard,
            defines: &defines,
            include: Some(&include),
        };
        let parsed = parse_with(text, &options);
        let root = parsed.syntax();
        assert_eq!(root.to_string(), *text, "round trip ({standard:?})");
        let len = text.len();
        for error in parsed.errors() {
            assert!(
                usize::from(error.range.end()) <= len,
                "error out of the text"
            );
        }
        for module in modules(&root) {
            assert!(usize::from(module.range.end()) <= len);
            for parameter in &module.parameters {
                let _ = parameter.declaration();
            }
            for port in &module.ports {
                let _ = port.declaration();
            }
        }
    }
});
