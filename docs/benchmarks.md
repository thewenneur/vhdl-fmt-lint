# Benchmarks against rust_hdl

`scripts/bench_rust_hdl.py` runs `vfl lint` and rust_hdl's `vhdl_lang` (both: full
analysis plus every lint check) on whole projects and reports the median wall time over
several runs, the peak memory, and the number of errors each tool reports inside the
project.

vfl runs with no configuration. rust_hdl cannot infer libraries, so it gets a
`vhdl_ls.toml` generated from vfl's inference (`vfl project --vhdl-ls`) and its own
standard libraries (`vhdl_libraries` from the same release).

## Results

rust_hdl 0.88.0 (`vhdl_lang` from crates.io) against this branch, release builds, 4-core
Intel Xeon at 2.8 GHz, Linux, 3 runs each. vfl now resolves overloads and checks types.

| Project | Files | vfl time | rust_hdl time | vfl memory | rust_hdl memory | vfl errors | rust_hdl errors |
|---|---:|---:|---:|---:|---:|---:|---:|
| neorv32 (b262045) | 68 | 140 ms | 356 ms | 30 MiB | 124 MiB | 0 | 0 |
| OSVVM (716d751) | 76 | 284 ms | 480 ms | 52 MiB | 185 MiB | 164 | 834 |
| UVVM (90d56e9) | 302 | 883 ms | 1809 ms | 127 MiB | 360 MiB | 5 | 162 |
| VUnit (98063a4) | 331 | 372 ms | 558 ms | 63 MiB | 191 MiB | 37 | 927 |

vfl is 1.5 to 2.5 times faster and uses 3 to 4 times less memory, without a library list.
Type checking costs about a third of vfl's time; it runs in parallel for every unit after
name resolution. The machine was noisier than for the previous table, where rust_hdl took
294, 345, 1189 and 436 ms.

What the remaining vfl errors are (none of them is a type error):

- OSVVM: 155 syntax errors are in files that are not VHDL-2008 (`*_template.vhd` with
  `$` placeholders, and the VHDL-2019 variants such as `DynamicVectorGenericPkg.vhd`);
  9 unresolved names are in stale demo files that call a removed `print`.
- UVVM: 4 testbenches instantiate DUTs that are not in the repository, and one template
  file (`td_vvc_entity_support_pkg.vhd`) refers to a name each VVC must provide.
- VUnit: all 37 refer to OSVVM, a git submodule that was not checked out.

rust_hdl reports the same real problems. Its extra errors are mostly:

- Cascades. When a unit is missing (VUnit's OSVVM submodule), rust_hdl reports every use
  of every name it would have declared (580 "no declaration" errors in VUnit) and 237
  circular dependencies. vfl reports the missing unit once and does not report names that
  could come from it.
- Duplicate units. The repositories ship alternate copies of some units (OSVVM's
  `deprecated/`, simulator-specific variants in UVVM). rust_hdl reports them as errors;
  vfl as `sema/duplicate-unit` warnings.
- Libraries that do not exist in the repository (UVVM templates): an error in rust_hdl,
  `sema/unknown-library` info in vfl.

### Type errors

On VUnit, rust_hdl reports 37 type and overload errors (8 "could not resolve call",
9 "name must denote a file", 9 "does not match file type", 9 "no association of
variable", 2 "expression must be a name denoting a file"). All are in VUnit's copy of
OSVVM's `AlertLogPkg.vhd` and come from the missing submodule: calls such as
`writeline(buf)` use overloads that OSVVM's `TranscriptPkg` declares. vfl reports none:
a call that may resolve to a declaration of a missing unit is not checked. On neorv32,
UVVM, OSVVM and the VESTS VHDL-93 suites, vfl reports no type error on valid code, except
in VESTS where several test files declare units of the same name (8 copies of entity
`reg` in one library) and an instantiation binds to another test's copy.

On `crates/vfl-sema/tests/data/types.vhd`, a fixture with 20 deliberate type errors
(wrong signal types, missing operators, bad record elements, unresolvable overloads,
invalid conversions, non-boolean conditions), both tools find all 20 at the same
places. On its valid part, rust_hdl reports 2 false errors that vfl does not:
`table(3)`, a call without arguments to a function whose array result is indexed
(LRM 9.3.4; GHDL's VESTS test tc1071 relies on it), and `sll` on a `bit_vector`.

### Editing

`crates/vfl-lsp/examples/edit_latency.rs` opens a project as the language server does,
then for 8 files spread over it makes 3 edits each and times the refresh, from the new
text to fresh diagnostics (inference, analysis and lint). "Inside a unit" appends a
comment; "type change" replaces the first `integer` with `natural` or `positive`.

| Project | Open | Inside a unit (median / max) | Type change (median / max) |
|---|---:|---:|---:|
| neorv32 | 110 ms | 7 / 11 ms | 6 / 9 ms |
| OSVVM | 220 ms | 8 / 68 ms | 15 / 76 ms |
| UVVM | 727 ms | 32 / 48 ms | 40 / 757 ms |
| VUnit | 365 ms | 16 / 100 ms | 16 / 25 ms |

Before, every edit re-analyzed and re-linted the whole project: a median of 92 ms on
neorv32, 181 ms on OSVVM, 778 ms on UVVM and 291 ms on VUnit. The slowest edits now
change a package that most of the project uses (UVVM's `types_pkg`), which re-analyzes
everything that depends on it.

## Running it

```sh
cargo build --release
cargo install vhdl_lang --root /tmp/rust_hdl
# rust_hdl's standard libraries: vhdl_libraries/ from the rust_hdl repository
scripts/bench_rust_hdl.py --vfl target/release/vfl \
    --vhdl-lang /tmp/rust_hdl/bin/vhdl_lang --libraries path/to/vhdl_libraries \
    --runs 5 path/to/neorv32 path/to/OSVVM path/to/UVVM path/to/vunit
```
