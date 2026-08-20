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
