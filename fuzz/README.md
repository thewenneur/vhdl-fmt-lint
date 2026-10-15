# Fuzzing

[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) targets for the
VHDL and Verilog parsers, the formatter, the linter, incremental
re-analysis and the language server. They live in their own workspace and need nightly Rust;
nothing here is built by the main workspace or ships in `vfl`.

| Target | Checks |
| --- | --- |
| `parse` | No panic; the tree gives back the input byte for byte, in 93, 2008 and 2019. |
| `verilog` | Same for Verilog and SystemVerilog, in every standard, with predefined macros and included files (cycles included); module summaries never panic. |
| `format` | No panic or internal error; formatting is idempotent; formatting one line and then the whole file gives what formatting the whole file gives. |
| `format_verilog` | Same for Verilog and SystemVerilog, in every standard. |
| `lint` | Analysis and every lint rule (strict preset) never panic on up to four VHDL and Verilog files; applying all fixes never breaks the syntax of a file that parsed. |
| `reanalyze` | After each edit to a VHDL or Verilog file, `Analysis::update` and `lint_cached` (what the language server uses) give exactly what a full analysis and lint give. |
| `lsp` | VHDL and Verilog documents edited with `didChange` events (positions out of range included), and every request answered at the edit; the final diagnostics equal those of a server that opened the final text. |

The multi-file targets read their input as files, each after a `\x01`
(VHDL) or `\x02` (Verilog), the first one VHDL unless the input starts
with `\x02`; then a `\0` and an edit script (see `src/lib.rs`). So a plain
VHDL file is a valid input. The `verilog` target reads the main file then
included files separated by `\x01`, named `inc0.vh`, `inc1.vh`...

```sh
cargo install cargo-fuzz
fuzz/seed.sh                      # seeds from examples/ and the test data
cd fuzz
cargo +nightly fuzz run -s none format corpus/format -- -dict=hdl.dict -max_len=16384
```

`-s none` skips AddressSanitizer: the workspace forbids `unsafe`, so it
finds nothing more and runs several times slower. Debug assertions, so
overflow checks, stay on. CI runs each target for 45 seconds.

A crash is saved in `fuzz/artifacts/<target>/`; replay it with
`cargo +nightly fuzz run -s none <target> <file>`, and shrink it with
`cargo +nightly fuzz tmin -s none <target> <file>`. Once fixed, add the
input as a regression test of the crate it was in.

For a long campaign, run the **Fuzz campaign** workflow from the Actions
tab: each target fuzzes in its own job, on every core, for `hours` (at
most 5.75, as GitHub stops a job after 6 hours), and the workflow starts
itself again until `rounds` runs are done; the defaults make about 12
hours. The corpus carries over between runs through the cache. Crashes,
timeouts and out-of-memory inputs don't stop it: each job uploads them as
`fuzz-artifacts-<target>` and prints them in base64 in its log.
