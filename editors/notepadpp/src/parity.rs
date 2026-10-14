//! Which commands of the plugin give each feature of the language server,
//! so that the plugin keeps up with it: the tests fail when the server
//! answers a request, or offers a kind of code action, that is not listed
//! here, or when a listed command is not documented.

/// Each language server request, code action kind and published
/// notification, with the plugin commands that give the same feature.
pub const FEATURES: &[(&str, &[&str])] = &[
    (
        "textDocument/formatting",
        &["Format document", "Format on save"],
    ),
    ("textDocument/rangeFormatting", &["Format selection"]),
    ("textDocument/onTypeFormatting", &["Format on type"]),
    ("textDocument/definition", &["Go to definition"]),
    ("textDocument/references", &["Find references"]),
    ("textDocument/documentHighlight", &["Highlight uses"]),
    ("textDocument/hover", &["Show type", "Code assistance"]),
    ("textDocument/documentSymbol", &["Go to symbol in file"]),
    ("textDocument/foldingRange", &["Fold code"]),
    ("workspace/symbol", &["Go to symbol in project"]),
    ("textDocument/prepareRename", &["Rename symbol"]),
    ("textDocument/rename", &["Rename symbol"]),
    (
        "textDocument/completion",
        &["Complete word", "Code assistance"],
    ),
    (
        "textDocument/signatureHelp",
        &["Show parameters", "Code assistance"],
    ),
    (
        "textDocument/semanticTokens/full",
        &["Color names by meaning"],
    ),
    (
        "textDocument/semanticTokens/full/delta",
        &["Color names by meaning"],
    ),
    (
        "textDocument/semanticTokens/range",
        &["Color names by meaning"],
    ),
    (
        "textDocument/codeAction",
        &["Fix problem at caret", "Fix all problems"],
    ),
    (
        "codeAction/resolve",
        &["Fix problem at caret", "Generate testbench"],
    ),
    ("quickfix", &["Fix problem at caret"]),
    ("source.fixAll.vfl", &["Fix all problems"]),
    (vfl_lsp::generate::TESTBENCH_KIND, &["Generate testbench"]),
    (
        "vfl/entities",
        &[
            "Insert entity instantiation",
            "Insert component instantiation",
            "Insert component declaration",
            "Generate testbench",
        ],
    ),
    (
        "vfl/generate",
        &[
            "Insert entity instantiation",
            "Insert component instantiation",
            "Insert component declaration",
            "Generate testbench",
        ],
    ),
    (
        "textDocument/publishDiagnostics",
        &[
            "Lint automatically",
            "Lint document",
            "List problems in project",
            "Show messages under lines",
        ],
    ),
    ("window/showMessage", &["Lint automatically"]),
    ("workspace/didChangeWatchedFiles", &["Reload project"]),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(name: &str) -> bool {
        FEATURES.iter().any(|(feature, _)| *feature == name)
    }

    /// The methods `handlers::request` answers, from its source.
    fn requests() -> Vec<&'static str> {
        let source = include_str!("../../../crates/vfl-lsp/src/handlers.rs");
        // Line by line: a Windows checkout may have CRLF line ends.
        source
            .lines()
            .skip_while(|line| !line.starts_with("pub fn request("))
            .take_while(|line| *line != "}")
            .filter_map(|line| {
                let rest = line.trim().strip_prefix('"')?;
                let (method, after) = rest.split_once('"')?;
                after.trim_start().starts_with("=>").then_some(method)
            })
            .collect()
    }

    #[test]
    fn every_request_of_the_server_has_a_command() {
        let requests = requests();
        assert!(requests.len() > 15, "{requests:?}");
        let missing: Vec<&str> = requests.into_iter().filter(|m| !listed(m)).collect();
        assert!(
            missing.is_empty(),
            "the language server answers {missing:?}, which the Notepad++ plugin does not offer"
        );
    }

    #[test]
    fn every_code_action_kind_has_a_command() {
        let capabilities = serde_json::to_value(vfl_lsp::capabilities()).unwrap();
        let kinds = capabilities["codeActionProvider"]["codeActionKinds"]
            .as_array()
            .expect("code action kinds");
        for kind in kinds {
            let kind = kind.as_str().unwrap();
            assert!(
                listed(kind),
                "code action kind {kind} has no Notepad++ command"
            );
        }
        // Every capability is backed by a request listed above.
        let object = capabilities.as_object().unwrap();
        for (name, value) in object {
            if value.is_null() || name == "textDocumentSync" {
                continue;
            }
            assert!(
                [
                    "documentFormattingProvider",
                    "documentRangeFormattingProvider",
                    "documentOnTypeFormattingProvider",
                    "definitionProvider",
                    "referencesProvider",
                    "hoverProvider",
                    "documentSymbolProvider",
                    "foldingRangeProvider",
                    "workspaceSymbolProvider",
                    "documentHighlightProvider",
                    "renameProvider",
                    "completionProvider",
                    "signatureHelpProvider",
                    "semanticTokensProvider",
                    "codeActionProvider",
                ]
                .contains(&name.as_str()),
                "new capability {name}: add it to the Notepad++ plugin and to FEATURES"
            );
        }
    }

    #[test]
    fn every_command_is_documented() {
        let readme = include_str!("../README.md");
        let documented: Vec<&str> = readme
            .lines()
            .filter_map(|line| line.strip_prefix("| ")?.split(" |").next())
            .flat_map(|cell| cell.split(", "))
            .collect();
        for (feature, commands) in FEATURES {
            assert!(!commands.is_empty(), "{feature}");
            for command in *commands {
                assert!(
                    documented.contains(command),
                    "{command} ({feature}) is not in the README's commands"
                );
            }
        }
    }
}
