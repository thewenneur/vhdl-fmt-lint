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
Intel Xeon at 2.8 GHz, Linux, median of 5 runs, peak resident memory. vfl resolves
overloads and checks types.

| Project | Files | vfl time | rust_hdl time | vfl memory | rust_hdl memory | vfl errors | rust_hdl errors |
|---|---:|---:|---:|---:|---:|---:|---:|
| neorv32 (b262045) | 68 | 156 ms | 362 ms | 30 MiB | 123 MiB | 0 | 0 |
| OSVVM (716d751) | 76 | 316 ms | 483 ms | 52 MiB | 185 MiB | 164 | 834 |
| UVVM (90d56e9) | 302 | 812 ms | 1688 ms | 128 MiB | 360 MiB | 5 | 162 |
| VUnit (98063a4) | 331 | 370 ms | 582 ms | 64 MiB | 191 MiB | 37 | 927 |

vfl is 1.5 to 2.3 times faster and uses 2.8 to 4.1 times less memory, without a library
list. Type checking costs about a third of vfl's time; it runs in parallel for every unit
after name resolution. A one-off run (the CLI's) skips the bookkeeping that incremental
re-analysis needs, so it costs the same time and memory as before it.

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
comment; "type change" replaces the first `integer` with `natural` or `positive`. Peak
memory is the whole process: opening the project, then the 48 edits.

| Project | Open | Inside a unit (median / max) | Type change (median / max) | Peak memory |
|---|---:|---:|---:|---:|
| neorv32 | 124 ms | 9 / 17 ms (was 103 / 124 ms) | 6 / 10 ms (was 109 / 134 ms) | 32 MiB (was 44 MiB) |
| OSVVM | 235 ms | 10 / 67 ms (was 180 / 243 ms) | 17 / 79 ms (was 213 / 241 ms) | 70 MiB (was 95 MiB) |
| UVVM | 715 ms | 30 / 55 ms (was 749 / 837 ms) | 45 / 828 ms (was 785 / 926 ms) | 201 MiB (was 213 MiB) |
| VUnit | 403 ms | 20 / 29 ms (was 305 / 358 ms) | 19 / 28 ms (was 286 / 311 ms) | 67 MiB (was 101 MiB) |

"Was" is the same program built from the previous version, which re-analyzed and
re-linted the whole project on every edit. Memory went down too: reused unit results are
shared between the old and the new analysis instead of being built again. The slowest
edits change a package that most of the project uses (UVVM's `types_pkg`), which
re-analyzes everything that depends on it.

### Editor requests

`crates/vfl-lsp/examples/request_latency.rs` opens a project, then sends hover, signature
help and completion at every 7th line of 16 files spread over it (the middle of the line).

| Project | Hover (median / max) | Signature help (median / max) | Completion (median / max) |
|---|---:|---:|---:|
| neorv32 | 0.07 / 2.6 ms | 0.01 / 0.9 ms | 0.9 / 3.5 ms |
| OSVVM | 0.2 / 10 ms | 0.02 / 1.4 ms | 1.5 / 13 ms |
| UVVM | 0.5 / 4.5 ms | 0.02 / 0.4 ms | 1.3 / 4.3 ms |
| VUnit | 0.3 / 2.1 ms | 0.02 / 1.8 ms | 1.2 / 7.5 ms |

It then asks for the semantic tokens of every file, whole and as a delta. On UVVM (302
files) the median is 0.1 ms; the slowest is `methods_pkg.vhd` (490 KB, 20,357 names):
7.4 ms whole, 4.8 ms as a delta.

## Running it

```sh
cargo build --release
cargo install vhdl_lang --root /tmp/rust_hdl
# rust_hdl's standard libraries: vhdl_libraries/ from the rust_hdl repository
scripts/bench_rust_hdl.py --vfl target/release/vfl \
    --vhdl-lang /tmp/rust_hdl/bin/vhdl_lang --libraries path/to/vhdl_libraries \
    --runs 5 path/to/neorv32 path/to/OSVVM path/to/UVVM path/to/vunit
```

## Language servers: vfl against vhdl_ls

`scripts/bench_vhdl_ls.py` starts `vfl lsp` and rust_hdl's `vhdl_ls` over stdio, drives
both with the same LSP client on the same project, and measures:

- **Open**: from `initialize` to the answer to the first request after `initialized`.
  Both servers analyze the whole project and publish its diagnostics before they answer.
- **Edits**: 8 files spread over the project, each opened, then edited 3 times by
  appending a comment and 3 times by changing the first `integer` to `natural` or
  `positive`, timed from `didChange` to the answer to the request that follows it (both
  servers re-analyze and publish diagnostics first).
- **Requests**: hover, completion and go to definition in the middle of the first
  identifier of every 7th line of 16 files spread over the project, with how many got a
  non-empty answer.
- **Peak memory** of the server process over the whole run.
- **Diagnostics** published for the project's files once it is open.

vfl runs with no configuration. vhdl_ls uses the project's own `vhdl_ls.toml` (UVVM has
one) or else one generated from vfl's inference, and its own standard libraries. Both
servers see the same files: the ones that `vhdl_ls.toml` lists (UVVM's lists 286 of its
302 files).

### Results

vhdl_ls 0.88.0 (from crates.io) against this branch, release builds, same machine as
above, 3 runs of each server (open time: median of the runs; latencies: median and
slowest over all runs).

| Project | Files | Open (vfl / vhdl_ls) | Peak memory (vfl / vhdl_ls) | Errors (vfl / vhdl_ls) | Warnings (vfl / vhdl_ls) |
|---|---:|---:|---:|---:|---:|
| neorv32 | 68 | 98 / 227 ms | 35 / 130 MiB | 0 / 0 | 10 / 18 |
| OSVVM | 76 | 197 / 346 ms | 81 / 227 MiB | 164 / 357 | 60 / 130 |
| UVVM | 286 | 604 / 1068 ms | 143 / 381 MiB | 5 / 41 | 98 / 555 |
| VUnit | 331 | 252 / 381 ms | 73 / 200 MiB | 37 / 1024 | 76 / 226 |

| Project | Server | Edit: comment (median / max) | Edit: type change (median / max) |
|---|---|---:|---:|
| neorv32 | vfl | 7.1 / 13 ms | 5.8 / 13 ms |
| neorv32 | vhdl_ls | 4.7 / 8.7 ms | 4.9 / 15 ms |
| OSVVM | vfl | 6.1 / 83 ms | 14 / 88 ms |
| OSVVM | vhdl_ls | 3.6 / 237 ms | 3.1 / 239 ms |
| UVVM | vfl | 13 / 28 ms | 13 / 130 ms |
| UVVM | vhdl_ls | 25 / 818 ms | 25 / 923 ms |
| VUnit | vfl | 7.3 / 20 ms | 7.1 / 16 ms |
| VUnit | vhdl_ls | 6.3 / 52 ms | 5.6 / 50 ms |

| Project | Server | Hover (median / max, answered) | Completion | Definition |
|---|---|---:|---:|---:|
| neorv32 | vfl | 0.31 / 4.2 ms, 296/303 | 2.8 / 8.7 ms, 303/303 | 0.44 / 4.9 ms, 278/303 |
| neorv32 | vhdl_ls | 15 / 44 ms, 302/303 | 3.6 / 49 ms, 230/303 | 0.48 / 21 ms, 285/303 |
| OSVVM | vfl | 0.42 / 5.5 ms, 1483/1532 | 2.7 / 32 ms, 1532/1532 | 0.50 / 16 ms, 1457/1532 |
| OSVVM | vhdl_ls | 18 / 44 ms, 1263/1532 | 6.8 / 73 ms, 1177/1532 | 0.84 / 36 ms, 1243/1532 |
| UVVM | vfl | 0.65 / 6.9 ms, 422/470 | 2.7 / 9.9 ms, 470/470 | 0.75 / 9.7 ms, 407/470 |
| UVVM | vhdl_ls | 22 / 66 ms, 469/470 | 6.3 / 18 ms, 419/470 | 0.68 / 67 ms, 454/470 |
| VUnit | vfl | 0.42 / 2.3 ms, 502/508 | 2.1 / 5.3 ms, 508/508 | 0.50 / 2.8 ms, 490/508 |
| VUnit | vhdl_ls | 8.3 / 36 ms, 505/508 | 5.0 / 20 ms, 489/508 | 0.64 / 34 ms, 493/508 |

What it shows:

- **Opening a project**: vfl is 1.5 to 2.3 times faster and uses 2.7 to 3.7 times less
  memory.
- **Edits**: a typical edit takes 6 to 14 ms in vfl and 3 to 25 ms in vhdl_ls. vhdl_ls is
  a few milliseconds faster on neorv32 and OSVVM, about the same on VUnit, and vfl is
  twice as fast on UVVM. vfl's slowest edits are much faster: 13 to 130 ms against
  50 ms to 0.9 s for vhdl_ls, whose worst case is an edit to a package most of the project
  depends on. A refresh in vfl infers libraries again only when the set of files, the
  units a file declares or the library names it refers to, or a library list
  (`vhdl_ls.toml`, tool files) changed; it re-lints only the files the analysis touched,
  and sends the editor only the diagnostics that changed. Before, it did all three on
  every edit, and took 7 to 26 ms at the median.
- **Requests**: vfl's hover is 20 to 50 times faster at the median (vhdl_ls takes 8 to
  22 ms per hover), completion 1.3 to 2.5 times, definition about the same at the median
  and 2 to 12 times at the slowest. vfl answers completion everywhere; hover and
  definition answer rates are close, vhdl_ls answering slightly more on UVVM and
  neorv32, vfl more on OSVVM (where vhdl_ls does not resolve names in the files it
  cannot parse or that refer to duplicate units).
- **Diagnostics**: the same picture as the CLI comparison above. vfl's 37 errors in
  VUnit are the missing OSVVM submodule reported once per use clause; vhdl_ls reports
  804 "no declaration" and 47 "could not resolve call" errors that cascade from it. In
  OSVVM, both report the `$` template files and the removed `print`; vhdl_ls also
  reports duplicate units as errors. On warnings, vhdl_ls reports every unused
  subprogram parameter (460 in UVVM), which vfl has no rule for, and unused ports and
  generics, which vfl reports only under the `strict` preset.

### Running it

```sh
cargo build --release
cargo install vhdl_ls --root /tmp/rust_hdl
# rust_hdl's standard libraries: vhdl_libraries/ from the rust_hdl repository
scripts/bench_vhdl_ls.py --vfl target/release/vfl \
    --vhdl-ls /tmp/rust_hdl/bin/vhdl_ls --libraries path/to/vhdl_libraries \
    --runs 3 path/to/neorv32 path/to/OSVVM path/to/UVVM path/to/vunit
```

It needs Python 3.11 or later, and reads peak memory from `/proc` (Linux).
