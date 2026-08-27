//! Vendor primitive declarations, generated from the checked-in libraries.
//!
//! Each library under `primitives/` is read by `build.rs` and emitted as a
//! module of [`rhdl::core::circuit::blackbox_decl::BlackBoxDecl`]
//! constants. A widget wrapping one of these modules calls
//! [`BlackBoxDecl::resolve`](rhdl::core::circuit::blackbox_decl::BlackBoxDecl::resolve)
//! to turn the port names into paths into its own `I` and `O`, and hands
//! the result to `with_netlist_black_box`.
//!
//! The declarations describe *whether* a path is combinational, never how
//! fast it is. See `black-box-connectivity.md`.

include!(concat!(env!("OUT_DIR"), "/xilinx_7series.rs"));
