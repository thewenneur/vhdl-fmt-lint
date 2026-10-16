//! What the reset checks share across languages: the names of reset
//! signals, and the consistency of the resets of a unit's clocked
//! processes. The VHDL checks (`rtl::reset`) and the Verilog ones (in
//! `vfl-lint`) find the resets in their own syntax, then use these.

use rowan::{TextRange, TextSize};

use crate::diagnostic::codes;

/// Whether a signal's name says it is a reset: `rst`, `arst_n`, `reset`.
pub fn is_reset_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("rst") || name.contains("reset")
}

/// Whether a reset's name says it is active low: `rst_n`, `resetn`,
/// `nrst`, `arst_l`, `rst_b`, and with `input_suffix` also `rst_ni` (the
/// SystemVerilog convention of suffixing inputs with `i`).
pub fn is_active_low_name(name: &str, input_suffix: bool) -> bool {
    let name = name.to_ascii_lowercase();
    ["_n", "_l", "_b", "rstn", "resetn"]
        .iter()
        .any(|s| name.ends_with(s))
        || (input_suffix && name.ends_with("_ni"))
        || ["nrst", "nreset", "n_rst", "n_reset"]
            .iter()
            .any(|p| name.starts_with(p))
}

/// The message for a reset named as active low but tested high, if it
/// is.
pub fn named_active_low(
    name: &str,
    active_high: Option<bool>,
    input_suffix: bool,
) -> Option<String> {
    (active_high == Some(true) && is_active_low_name(name, input_suffix))
        .then(|| format!("`{name}` is named as active low but resets when high here"))
}

/// One reset of one clocked process.
pub struct ResetUse<K> {
    /// What tells the reset signal from others: its declaration, or its
    /// name.
    pub key: K,
    /// Its name as written, for messages.
    pub name: String,
    /// Tested in the clocked branch rather than before it.
    pub sync: bool,
    /// Whether the reset is active when the signal is high; `None` when
    /// its level is a constant or a parameter.
    pub active_high: Option<bool>,
    /// The range of the reset signal's name in the test.
    pub range: TextRange,
    /// Inside an `if` or `case` generate, where alternatives are expected
    /// to differ.
    pub in_generate: bool,
}

/// A finding of [`consistency`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inconsistency {
    pub code: &'static str,
    pub range: TextRange,
    pub message: String,
}

/// Reset signals used both synchronously and asynchronously
/// (`sema/mixed-reset`, with `mixed`), or tested with both polarities
/// (`sema/reset-polarity`, with `polarity`), within a unit. `process` is
/// what the language calls a process (`process`, `block`), and `line`
/// gives the line of an offset.
pub fn consistency<K: Eq>(
    uses: &[ResetUse<K>],
    process: &str,
    line: impl Fn(TextSize) -> usize,
    mixed: bool,
    polarity: bool,
) -> Vec<Inconsistency> {
    let mut by_signal: Vec<Vec<&ResetUse<K>>> = Vec::new();
    for u in uses.iter().filter(|u| !u.in_generate) {
        match by_signal.iter_mut().find(|list| list[0].key == u.key) {
            Some(list) => list.push(u),
            None => by_signal.push(vec![u]),
        }
    }
    by_signal.sort_by_key(|list| list[0].range.start());
    let line = |u: &ResetUse<K>| line(u.range.start());
    let mut out = Vec::new();
    for uses in by_signal {
        let first = uses[0];
        let name = &first.name;
        if mixed && let Some(other) = uses.iter().find(|u| u.sync != first.sync) {
            let describe = |sync: bool| {
                if sync {
                    "synchronously"
                } else {
                    "asynchronously"
                }
            };
            out.push(Inconsistency {
                code: codes::MIXED_RESET,
                range: other.range,
                message: format!(
                    "`{name}` resets this {process} {} but the {process} at line {} {}",
                    describe(other.sync),
                    line(first),
                    describe(first.sync)
                ),
            });
        }
        if !polarity {
            continue;
        }
        let Some(level) = uses.iter().find_map(|u| u.active_high) else {
            continue;
        };
        let first = uses.iter().find(|u| u.active_high == Some(level)).unwrap();
        for other in uses.iter().filter(|u| u.active_high == Some(!level)) {
            let describe = |high: bool| if high { "high" } else { "low" };
            out.push(Inconsistency {
                code: codes::RESET_POLARITY,
                range: other.range,
                message: format!(
                    "`{name}` resets when {} here but when {} at line {}",
                    describe(!level),
                    describe(level),
                    line(first)
                ),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_names() {
        for name in ["rst", "RST_N", "arst", "reset", "sys_reset_n", "nrst"] {
            assert!(is_reset_name(name), "{name}");
        }
        for name in ["clk", "en", "valid"] {
            assert!(!is_reset_name(name), "{name}");
        }
        for name in [
            "rst_n", "resetn", "RSTN", "nrst", "n_reset", "arst_l", "rst_b",
        ] {
            assert!(is_active_low_name(name, false), "{name}");
        }
        for name in ["rst", "reset", "rst_sync", "rst_ni"] {
            assert!(!is_active_low_name(name, false), "{name}");
        }
        assert!(is_active_low_name("rst_ni", true));
        assert_eq!(
            named_active_low("rst_n", Some(true), false).as_deref(),
            Some("`rst_n` is named as active low but resets when high here")
        );
        assert_eq!(named_active_low("rst_n", Some(false), false), None);
        assert_eq!(named_active_low("rst_n", None, false), None);
        assert_eq!(named_active_low("rst", Some(true), false), None);
    }

    fn reset(key: u32, at: u32, sync: bool, active_high: Option<bool>) -> ResetUse<u32> {
        ResetUse {
            key,
            name: format!("r{key}"),
            sync,
            active_high,
            range: TextRange::at(at.into(), 2.into()),
            in_generate: false,
        }
    }

    #[test]
    fn mixed_and_polarity() {
        let uses = [
            reset(1, 30, false, Some(true)),
            reset(0, 10, true, Some(false)),
            reset(0, 20, false, None),
            reset(1, 40, false, Some(false)),
            reset(0, 50, true, Some(true)),
        ];
        let line = |at: TextSize| u32::from(at) as usize / 10;
        let found = consistency(&uses, "block", line, true, true);
        let found: Vec<(&str, u32, &str)> = found
            .iter()
            .map(|i| (i.code, u32::from(i.range.start()), i.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (
                    codes::MIXED_RESET,
                    20,
                    "`r0` resets this block asynchronously but the block at line 1 synchronously"
                ),
                (
                    codes::RESET_POLARITY,
                    50,
                    "`r0` resets when high here but when low at line 1"
                ),
                (
                    codes::RESET_POLARITY,
                    40,
                    "`r1` resets when low here but when high at line 3"
                ),
            ]
        );
        assert_eq!(consistency(&uses, "block", line, false, true).len(), 2);
        assert_eq!(consistency(&uses, "block", line, true, false).len(), 1);
    }

    #[test]
    fn generate_alternatives_are_left_out() {
        let mut uses = [
            reset(0, 10, true, Some(true)),
            reset(0, 20, false, Some(false)),
        ];
        uses[1].in_generate = true;
        assert!(consistency(&uses, "process", |_| 1, true, true).is_empty());
    }
}
