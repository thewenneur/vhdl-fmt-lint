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
Intel Xeon at 2.8 GHz, Linux, 3 runs each.

| Project | Files | vfl time | rust_hdl time | vfl memory | rust_hdl memory | vfl errors | rust_hdl errors |
|---|---:|---:|---:|---:|---:|---:|---:|
| neorv32 (b262045) | 68 | 95 ms | 294 ms | 25 MiB | 123 MiB | 0 | 0 |
| OSVVM (716d751) | 76 | 175 ms | 345 ms | 45 MiB | 185 MiB | 164 | 340 |
| UVVM (90d56e9) | 302 | 451 ms | 1189 ms | 103 MiB | 361 MiB | 5 | 111 |
| VUnit (98063a4) | 331 | 215 ms | 436 ms | 53 MiB | 195 MiB | 37 | 968 |

vfl is 2 to 3 times faster and uses 3 to 5 times less memory, without a library list.

What the remaining vfl errors are:

- OSVVM: 155 syntax errors are in files that are not VHDL-2008 (`*_template.vhd` with
  `$` placeholders, and the VHDL-2019 variants such as `DynamicVectorGenericPkg.vhd`);
  9 unresolved names are in stale demo files that call a removed `print`.
- UVVM: 4 testbenches instantiate DUTs that are not in the repository, and one template
  file (`td_vvc_entity_support_pkg.vhd`) refers to a name each VVC must provide.
- VUnit: all 37 refer to OSVVM, a git submodule that was not checked out.

rust_hdl reports the same real problems. Its extra errors are mostly:

- Cascades. When a unit is missing (VUnit's OSVVM submodule), rust_hdl reports every use
  of every name it would have declared (159 uses of `rnd`, 120 of `OsvvmOptionsType`, ...).
  vfl reports the missing unit once and does not report names that could come from it.
- Duplicate units. The repositories ship alternate copies of some units (OSVVM's
  `deprecated/`, simulator-specific variants in UVVM). rust_hdl reports them as errors;
  vfl as `sema/duplicate-unit` warnings.
- Libraries that do not exist in the repository (UVVM templates): an error in rust_hdl,
  `sema/unknown-library` info in vfl.
- Type checks. rust_hdl resolves overloads and checks types, which vfl does not do yet, so
  some of its errors (argument counts, subtype mismatches) are checks vfl does not make.
  Those were not verified one by one, so the error columns compare noise on valid code
  more than they compare checking power.

## Running it

```sh
cargo build --release
cargo install vhdl_lang --root /tmp/rust_hdl
# rust_hdl's standard libraries: vhdl_libraries/ from the rust_hdl repository
scripts/bench_rust_hdl.py --vfl target/release/vfl \
    --vhdl-lang /tmp/rust_hdl/bin/vhdl_lang --libraries path/to/vhdl_libraries \
    --runs 5 path/to/neorv32 path/to/OSVVM path/to/UVVM path/to/vunit
```
