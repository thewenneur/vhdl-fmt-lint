//! Near misses: the known name a misspelled one was meant to be.

/// The edit distance of `a` and `b` (insertions, deletions,
/// substitutions and swaps of neighbors), ignoring case, or `None` when
/// over `limit`.
fn distance(a: &str, b: &str, limit: usize) -> Option<usize> {
    let a: Vec<u8> = a.bytes().map(|c| c.to_ascii_lowercase()).collect();
    let b: Vec<u8> = b.bytes().map(|c| c.to_ascii_lowercase()).collect();
    if a.len().abs_diff(b.len()) > limit {
        return None;
    }
    let mut previous2 = vec![0usize; b.len() + 1];
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        current[0] = i;
        let mut best = current[0];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            current[j] = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                current[j] = current[j].min(previous2[j - 2] + 1);
            }
            best = best.min(current[j]);
        }
        if best > limit {
            return None;
        }
        std::mem::swap(&mut previous2, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    let d = previous[b.len()];
    (d <= limit).then_some(d)
}

/// The known name `name` is a near miss of, if any: within one edit for
/// short names, two for longer ones.
pub fn near<'k>(name: &str, known: impl IntoIterator<Item = &'k str>) -> Option<&'k str> {
    let limit = if name.len() < 6 { 1 } else { 2 };
    let mut best: Option<(usize, &str)> = None;
    for candidate in known {
        if candidate.eq_ignore_ascii_case(name) {
            return None;
        }
        if let Some(d) = distance(name, candidate, limit)
            && best.is_none_or(|(b, _)| d < b)
            && !numbered(name, candidate)
        {
            best = Some((d, candidate));
        }
    }
    best.map(|(_, c)| c)
}

/// Whether `a` and `b` differ only in digits, such as `DEVICE1_ADDRESS`
/// and `DEVICE0_ADDRESS`: one of a numbered family, not a typo.
fn numbered(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .all(|(x, y)| x.eq_ignore_ascii_case(&y) || x.is_ascii_digit() && y.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_misses() {
        let known = ["create_clock", "set_false_path", "VHDL_FILE"];
        assert_eq!(near("create_clok", known), Some("create_clock"));
        assert_eq!(near("set_flase_path", known), Some("set_false_path"));
        assert_eq!(near("VHLD_FILE", known), Some("VHDL_FILE"));
        assert_eq!(near("vhdl_file", known), None);
        assert_eq!(near("totally_else", known), None);
        assert_eq!(
            near("PWRMGT_DEVICE1_ADDRESS", ["PWRMGT_DEVICE0_ADDRESS"]),
            None
        );
    }
}
