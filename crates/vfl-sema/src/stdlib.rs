//! The bundled `std` and `ieee` libraries.
//!
//! `std` is written for this tool from the LRM. The `ieee` packages are the
//! IEEE P1076 open-source package declarations (Apache-2.0, see
//! `stdlib/ieee/LICENSE`) and the Synopsys `std_logic_arith` family, which
//! may be redistributed with its copyright notice. Only declarations are
//! bundled: name resolution never needs the bodies.

/// `(library, file name, text)` of every bundled source.
pub const SOURCES: &[(&str, &str, &str)] = &[
    (
        "std",
        "standard.vhd",
        include_str!("../stdlib/std/standard.vhd"),
    ),
    (
        "std",
        "textio.vhd",
        include_str!("../stdlib/std/textio.vhd"),
    ),
    ("std", "env.vhd", include_str!("../stdlib/std/env.vhd")),
    ieee(
        "std_logic_1164.vhdl",
        include_str!("../stdlib/ieee/std_logic_1164.vhdl"),
    ),
    ieee(
        "numeric_std.vhdl",
        include_str!("../stdlib/ieee/numeric_std.vhdl"),
    ),
    ieee(
        "numeric_bit.vhdl",
        include_str!("../stdlib/ieee/numeric_bit.vhdl"),
    ),
    ieee(
        "numeric_std_unsigned.vhdl",
        include_str!("../stdlib/ieee/numeric_std_unsigned.vhdl"),
    ),
    ieee(
        "numeric_bit_unsigned.vhdl",
        include_str!("../stdlib/ieee/numeric_bit_unsigned.vhdl"),
    ),
    ieee(
        "math_real.vhdl",
        include_str!("../stdlib/ieee/math_real.vhdl"),
    ),
    ieee(
        "math_complex.vhdl",
        include_str!("../stdlib/ieee/math_complex.vhdl"),
    ),
    ieee(
        "fixed_float_types.vhdl",
        include_str!("../stdlib/ieee/fixed_float_types.vhdl"),
    ),
    ieee(
        "fixed_generic_pkg.vhdl",
        include_str!("../stdlib/ieee/fixed_generic_pkg.vhdl"),
    ),
    ieee(
        "fixed_pkg.vhdl",
        include_str!("../stdlib/ieee/fixed_pkg.vhdl"),
    ),
    ieee(
        "float_generic_pkg.vhdl",
        include_str!("../stdlib/ieee/float_generic_pkg.vhdl"),
    ),
    ieee(
        "float_pkg.vhdl",
        include_str!("../stdlib/ieee/float_pkg.vhdl"),
    ),
    ieee(
        "ieee_bit_context.vhdl",
        include_str!("../stdlib/ieee/ieee_bit_context.vhdl"),
    ),
    ieee(
        "ieee_std_context.vhdl",
        include_str!("../stdlib/ieee/ieee_std_context.vhdl"),
    ),
    ieee(
        "std_logic_textio.vhdl",
        include_str!("../stdlib/ieee/std_logic_textio.vhdl"),
    ),
    ieee(
        "std_logic_arith.vhdl",
        include_str!("../stdlib/ieee/std_logic_arith.vhdl"),
    ),
    ieee(
        "std_logic_unsigned.vhdl",
        include_str!("../stdlib/ieee/std_logic_unsigned.vhdl"),
    ),
    ieee(
        "std_logic_signed.vhdl",
        include_str!("../stdlib/ieee/std_logic_signed.vhdl"),
    ),
    ieee(
        "std_logic_misc.vhdl",
        include_str!("../stdlib/ieee/std_logic_misc.vhdl"),
    ),
];

const fn ieee(
    name: &'static str,
    text: &'static str,
) -> (&'static str, &'static str, &'static str) {
    ("ieee", name, text)
}

/// Units of bundled libraries that are not bundled (VITAL, mostly). Names
/// from them are not checked rather than reported.
pub const OPAQUE_UNITS: &[(&str, &str)] = &[
    ("ieee", "vital_timing"),
    ("ieee", "vital_primitives"),
    ("ieee", "vital_memory"),
];

/// `(library, file name, text)` of the vendor primitive stubs: component
/// declarations of the primitives most designs instantiate, with their
/// ports and the generics most often set, written for this tool from the
/// vendors' public documentation (no vendor source is copied). A library's
/// stubs are only analyzed when the design uses it and has no copy of its
/// own; what they do not declare is not checked (see [`VENDOR_LIBRARIES`]).
pub const VENDOR: &[(&str, &str, &str)] = &[
    (
        "unisim",
        "vcomponents.vhd",
        include_str!("../stdlib/vendor/unisim/vcomponents.vhd"),
    ),
    (
        "xpm",
        "vcomponents.vhd",
        include_str!("../stdlib/vendor/xpm/vcomponents.vhd"),
    ),
    (
        "altera_mf",
        "altera_mf_components.vhd",
        include_str!("../stdlib/vendor/altera_mf/altera_mf_components.vhd"),
    ),
    (
        "ecp5u",
        "components.vhd",
        include_str!("../stdlib/vendor/ecp5u/components.vhd"),
    ),
    (
        "machxo2",
        "components.vhd",
        include_str!("../stdlib/vendor/machxo2/components.vhd"),
    ),
    (
        "sb_ice40_components_syn",
        "components.vhd",
        include_str!("../stdlib/vendor/sb_ice40_components_syn/components.vhd"),
    ),
];

/// The libraries [`VENDOR`] has stubs for.
pub const VENDOR_LIBRARIES: &[&str] = &[
    "unisim",
    "xpm",
    "altera_mf",
    "ecp5u",
    "machxo2",
    "sb_ice40_components_syn",
];
