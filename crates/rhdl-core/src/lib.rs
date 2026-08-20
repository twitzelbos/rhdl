//! The RHDL Core Library
//#![warn(missing_docs)]
#![deny(unsafe_code)]
#![deny(unused_must_use)]
pub use circuit::circuit_impl::Circuit;
pub use circuit::circuit_impl::CircuitDQ;
pub use circuit::circuit_impl::CircuitIO;
pub use circuit::descriptor::AsyncKind;
pub use circuit::descriptor::Descriptor;
pub use circuit::descriptor::SyncKind;
pub use circuit::hdl_descriptor::HDLDescriptor;
pub use circuit::synchronous::Synchronous;
pub use circuit::synchronous::SynchronousDQ;
pub use circuit::synchronous::SynchronousIO;
pub use types::clock::Clock;
pub use types::digital::Digital;
pub use types::digital_fn::DigitalFn;
pub use types::digital_fn::DigitalFn2;
pub use types::digital_fn::DigitalFn3;
pub use types::domain::Color;
pub use types::domain::Domain;
pub use types::kernel::KernelFnKind;
pub use types::kind::DiscriminantAlignment;
pub use types::kind::Kind;
pub use types::reset::Reset;
pub use types::reset_n::ResetN;
pub use types::signal::Signal;
pub use types::timed::Timed;
pub mod ast;
pub mod circuit;
pub mod compiler;
pub mod fsm;
pub mod types;
pub mod util;
pub use util::id;

pub use compiler::compile_design;
pub use trace::key::TraceKey;
pub use trace::page::trace;
pub use trace::page::trace_pop_path;
pub use trace::page::trace_push_path;
pub use types::kind::DiscriminantType;
pub use types::typed_bits::TypedBits;
pub mod rhif;
pub use ast::builder;
pub use types::clock;
pub use types::digital_fn;
pub use types::digital_fn::DigitalSignature;
pub use types::kernel;

pub const MAX_ITERS: usize = 10;
pub mod error;
pub use error::RHDLError;
pub mod rtl;
pub use compiler::CompilationMode;
pub use types::clock_reset::ClockReset;
pub use types::clock_reset::clock_reset;

pub mod sim;
pub use types::timed_sample::TimedSample;
pub use types::timed_sample::timed_sample;
pub mod hdl;
pub use bitx::dyn_bit_manip::move_nbits_to_msb;
pub use rhdl_trace_type::TraceType;
pub use trace::rtt;
pub mod bitx;
pub use bitx::BitX;
pub use bitx::bitx_vec;
pub mod common;
pub mod ntl;
pub use circuit::scoped_name::ScopedName;
pub mod trace;

#[cfg(test)]
mod iverilog_precondition {
    //! Icarus Verilog is a required precondition, not an optional tool.
    //!
    //! Tier 4 of the validation stack compiles this crate's emitted Verilog
    //! and simulates it against the Rust model. Without a working
    //! `iverilog` that check does not run, so the suite would report
    //! success while proving much less than it appears to.
    //!
    //! One test per crate, because `require_iverilog` exits the test
    //! process: whichever crate cargo reaches first aborts the run with a
    //! single actionable message instead of a few hundred `NotFound`
    //! panics that all share the one cause.

    #[test]
    fn a_working_iverilog_is_present() {
        rhdl_vlog::toolchain::require_iverilog();
    }
}
