#![warn(missing_docs)]
//! Design-time DSP mathematics — no hardware, and no RHDL dependency.
//!
//! Everything here answers a question about a filter *before* any of it
//! is built: what a CIC does to the passband, how many bits each stage
//! needs, which decimation split is cheaper, what taps undo the droop.
//! None of it knows about `Digital`, widgets, or Verilog.
//!
//! # Why this is its own crate
//!
//! Two consumers that cannot share one.
//!
//! [`rhdl_fpga`](https://docs.rs/rhdl-fpga) needs it at runtime, to
//! size widgets and to check the parameters it was given. A future
//! `cic_chain!` proc macro needs it at *expansion* time, to turn
//! requirements into the const-generic parameters a widget takes — and
//! proc macros live in `rhdl-macro-core`, which
//! [architecture.md §2](https://github.com/twitzelbos/rhdl) forbids
//! from depending on `rhdl-core`, because a macro crate that depends on
//! the runtime it generates code for creates build cycles and wrecks
//! incremental compilation.
//!
//! So the math has to live somewhere both can reach. It has no RHDL
//! dependency of its own — it is `f64` and integer arithmetic — so a
//! leaf crate is the honest place for it. `architecture.md` §5 defaults
//! against new crates and asks for a justification; this is it, and the
//! alternative was putting filter design inside `rhdl-bits` or
//! `rhdl-vlog`, which would be worse drift than one leaf crate.
//!
//! `rhdl-fpga` re-exports this module tree from `dsp::cic`, so callers
//! and the `cic_pruned!` macro see no difference.

pub mod cic;
pub mod fir;
