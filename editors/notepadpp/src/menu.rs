//! The entries of the **Plugins > vfl** menu, in order.
//!
//! Notepad++ lists every entry in **Settings > Shortcut Mapper > Plugin
//! commands**, where it can be given a key, and saves that key in
//! `shortcuts.xml` under the entry's position in this list (its
//! `internalID`). Moving an entry, or inserting one before it, would move
//! the user's keys to other commands, so entries are only ever added at the
//! end; the test below holds every released position in place.

/// The menu entries; `""` is a separator. The plugin pairs each with its
/// command, in the same order.
pub const NAMES: &[&str] = &[
    "Format document",
    "Format selection",
    "Lint document",
    "Next problem",
    "Previous problem",
    "List problems in project",
    "Fix problem at caret",
    "Fix all problems",
    "Clear problems",
    "",
    "Go to definition",
    "Go back",
    "Find references",
    "Rename symbol",
    "Go to symbol in file",
    "Go to symbol in project",
    "",
    "Insert entity instantiation",
    "Insert component instantiation",
    "Insert component declaration",
    "Generate testbench",
    "",
    "Generate documentation",
    "Show diagrams",
    "",
    "Show type",
    "Show parameters",
    "Complete word",
    "",
    "Format on save",
    "Format on type",
    "Lint automatically",
    "Show messages under lines",
    "Code assistance",
    "Highlight uses",
    "Color names by meaning",
    "",
    "Reload project",
    "About",
    "",
    "Lint preset without vfl.toml...",
    "Format style without vfl.toml...",
    "Generate project config (vfl.toml)",
];

#[cfg(test)]
mod tests {
    use super::NAMES;

    /// The menu of v0.6.0. Users' shortcuts point at these positions.
    /// Renaming an entry keeps its shortcut; moving or removing one does
    /// not.
    const RELEASED: &[&str] = &[
        "Format document",
        "Format selection",
        "Lint document",
        "Next problem",
        "Previous problem",
        "List problems in project",
        "Fix problem at caret",
        "Fix all problems",
        "Clear problems",
        "",
        "Go to definition",
        "Go back",
        "Find references",
        "Rename symbol",
        "Go to symbol in file",
        "Go to symbol in project",
        "",
        "Insert entity instantiation",
        "Insert component instantiation",
        "Insert component declaration",
        "Generate testbench",
        "",
        "Generate documentation",
        "Show diagrams",
        "",
        "Show type",
        "Show parameters",
        "Complete word",
        "",
        "Format on save",
        "Format on type",
        "Lint automatically",
        "Show messages under lines",
        "Code assistance",
        "Highlight uses",
        "Color names by meaning",
        "",
        "Reload project",
        "About",
    ];

    #[test]
    fn released_entries_keep_their_position() {
        assert!(
            NAMES.len() >= RELEASED.len(),
            "a released entry was removed"
        );
        for (i, (now, then)) in NAMES.iter().zip(RELEASED).enumerate() {
            assert_eq!(
                now, then,
                "entry {i} moved: add new entries at the end, or Notepad++ \
                 gives users' shortcuts to other commands"
            );
        }
    }

    #[test]
    fn names_fit_the_menu() {
        for name in NAMES {
            // MENU_ITEM_SIZE is 64 UTF-16 units, with the terminating zero.
            assert!(name.encode_utf16().count() < 64, "{name}");
        }
        let mut seen = std::collections::HashSet::new();
        for name in NAMES.iter().filter(|n| !n.is_empty()) {
            assert!(seen.insert(name), "{name} twice");
        }
    }
}
