//! Lossless, error-tolerant VHDL lexer and parser.
//!
//! The parser is not written yet; this crate currently only defines the
//! language standards it will support. See `docs/design.md`.

use std::fmt;
use std::str::FromStr;

/// A VHDL language revision.
///
/// The parser accepts constructs from every revision and reports the ones
/// that need a newer standard than the selected one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Standard {
    /// IEEE 1076-1993.
    Vhdl1993,
    /// IEEE 1076-2008.
    #[default]
    Vhdl2008,
    /// IEEE 1076-2019.
    Vhdl2019,
}

impl Standard {
    pub const ALL: [Standard; 3] = [Standard::Vhdl1993, Standard::Vhdl2008, Standard::Vhdl2019];

    /// The short name used in configuration files and on the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            Standard::Vhdl1993 => "93",
            Standard::Vhdl2008 => "2008",
            Standard::Vhdl2019 => "2019",
        }
    }
}

impl fmt::Display for Standard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownStandard(pub String);

impl fmt::Display for UnknownStandard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown VHDL standard `{}` (expected 93, 2008 or 2019)",
            self.0
        )
    }
}

impl std::error::Error for UnknownStandard {}

impl FromStr for Standard {
    type Err = UnknownStandard;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "93" | "1993" => Ok(Standard::Vhdl1993),
            "08" | "2008" => Ok(Standard::Vhdl2008),
            "19" | "2019" => Ok(Standard::Vhdl2019),
            _ => Err(UnknownStandard(s.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_round_trips_through_its_name() {
        for std in Standard::ALL {
            assert_eq!(std.as_str().parse::<Standard>(), Ok(std));
        }
    }

    #[test]
    fn standards_are_ordered_by_age() {
        assert!(Standard::Vhdl1993 < Standard::Vhdl2008);
        assert!(Standard::Vhdl2008 < Standard::Vhdl2019);
    }
}
