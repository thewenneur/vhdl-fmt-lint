# Fuzzing

[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) targets for the
parser, the formatter, the linter, incremental re-analysis and the
language server. They live in their own workspace and need nightly Rust;
nothing here is built by the main workspace or ships in `vfl`.

| Target | Checks |
| --- | --- |
| `parse` | No panic; the tree gives back the input byte for byte, in 93, 2008 and 2019. |
| `format` | No panic or internal error; formatting is idempotent; formatting one line and then the whole file gives what formatting the whole file gives. |
| `lint` | Analysis and every lint rule (strict preset) never panic on up to four files; applying all fixes never breaks the syntax of a file that parsed. |
| `reanalyze` | After each edit, `Analysis::update` and `lint_cached` (what the language server uses) give exactly what a full analysis and lint give. |
| `lsp` | Documents edited with `didChange` events (positions out of range included), and every request answered at the edit; the final diagnostics equal those of a server that opened the final text. |

The multi-file targets read their input as files separated by `\x01`,
then a `\0` and an edit script (see `src/lib.rs`), so a plain VHDL file is
a valid input.

```sh
cargo install cargo-fuzz
fuzz/seed.sh                      # seeds from examples/ and the test data
cd fuzz
cargo +nightly fuzz run -s none format corpus/format -- -dict=vhdl.dict -max_len=16384
```

`-s none` skips AddressSanitizer: the workspace forbids `unsafe`, so it
finds nothing more and runs several times slower. Debug assertions, so
overflow checks, stay on. CI runs each target for 45 seconds.

A crash is saved in `fuzz/artifacts/<target>/`; replay it with
`cargo +nightly fuzz run -s none <target> <file>`, and shrink it with
`cargo +nightly fuzz tmin -s none <target> <file>`. Once fixed, add the
input as a regression test of the crate it was in.
