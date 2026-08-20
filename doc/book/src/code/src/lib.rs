/// Skip the calling test when the IceStorm toolchain is not installed.
///
/// Returns `Ok(())` early with a note on stderr, so the test **runs** where
/// yosys/nextpnr/icetime are present and **skips** where they are not.
/// See [`toolchain`] for why this is a runtime check rather than a blanket
/// `#[ignore]`.
#[macro_export]
macro_rules! skip_without_icestorm {
    () => {
        if !$crate::toolchain::icestorm_available() {
            eprintln!(
                "skipping: IceStorm toolchain not installed (missing: {})",
                $crate::toolchain::missing_icestorm_tools().join(", ")
            );
            return Ok(());
        }
    };
}

pub mod bits;
pub mod circuits;
pub mod count_ones;
pub mod digital;
pub mod fixturing;
pub mod half_adder;
pub mod kernels;
pub mod probes;
pub mod simulations;
pub mod synchronous;
pub mod testbench;
pub mod timed;
pub mod toolchain;
pub mod trace;
pub mod xor;

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
