//! FPGA Support for RHDL
pub mod audio;
pub mod axi4lite;
pub mod cdc;
pub mod core;
pub mod doc;
pub mod dsp;
pub mod fifo;
pub mod gray;
pub mod lid;
pub mod pipe;
pub mod rcstream;
pub mod reset;
pub mod rng;
pub mod serial_bus;
pub mod stream;
/// Tristate IO support
pub mod tristate;
pub mod video;

#[cfg(test)]
mod fsm_corpus_regression;
#[cfg(test)]
mod widget_property_corpus;
#[cfg(test)]
mod widget_well_formedness;

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
        rhdl::vlog::toolchain::require_iverilog();
    }
}
