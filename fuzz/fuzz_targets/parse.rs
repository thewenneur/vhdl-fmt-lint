//! The parser never panics and its tree always gives back the input,
//! byte for byte, in every standard.
#![no_main]

use libfuzzer_sys::fuzz_target;
use vfl_syntax::{Standard, parse};

fuzz_target!(|data: &[u8]| {
    let text = vfl_fuzz::text(data);
    for std in [Standard::Vhdl1993, Standard::Vhdl2008, Standard::Vhdl2019] {
        let parsed = parse(&text, std);
        assert_eq!(parsed.syntax().to_string(), text, "round trip ({std:?})");
        for error in parsed.errors() {
            assert!(
                usize::from(error.range.end()) <= text.len(),
                "error out of the text"
            );
        }
    }
});
