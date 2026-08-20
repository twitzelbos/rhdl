//! External-toolchain availability checks.
//!
//! Some tests in this crate drive real FPGA tooling — yosys, nextpnr and
//! the IceStorm utilities. Those tests should **run** where the tools are
//! installed and **skip** where they are not. Doing neither is what caused
//! the problem this module exists to fix: three `count_ones` timing tests
//! failed outright on any machine without the toolchain, and because
//! `cargo test` is fail-fast across test *binaries*, they aborted every
//! workspace run before `rhdl-fpga` was reached. Two unrelated defects hid
//! behind that for weeks.
//!
//! A blanket `#[ignore]` fixes the failure but throws away the coverage
//! everywhere, and CLAUDE.md calls that "a temporary measure, not a
//! permanent state". Hence a runtime check.
//!
//! # What this does not cover
//!
//! Tests that need a **board physically attached** (anything reaching
//! `iceprog`, such as `test_build_flash`) stay `#[ignore]`d. No PATH lookup
//! can detect a USB cable, and that requirement is permanent rather than
//! environmental.

use std::path::PathBuf;

/// Whether `name` is an executable on `PATH`.
///
/// A plain `PATH` walk rather than shelling out to `which`, so the check
/// costs no process spawn and behaves the same on any platform with a
/// `PATH`.
pub fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let candidate: PathBuf = dir.join(name);
                candidate.is_file()
            })
        })
        .unwrap_or(false)
}

/// The binaries an IceStorm synthesis-and-timing run needs.
///
/// `icepack` is included because [`crate::count_ones`]'s bitstream steps
/// reach it; `iceprog` is not, because that needs hardware.
pub const ICESTORM_TOOLS: &[&str] = &["yosys", "nextpnr-ice40", "icetime", "icepack"];

/// Whether a full IceStorm place-and-route + timing run is possible here.
pub fn icestorm_available() -> bool {
    ICESTORM_TOOLS.iter().all(|t| on_path(t))
}

/// Names of the missing IceStorm tools, for a useful skip message.
pub fn missing_icestorm_tools() -> Vec<&'static str> {
    ICESTORM_TOOLS
        .iter()
        .copied()
        .filter(|t| !on_path(t))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The check finds something that is definitely present and rejects
    /// something that definitely is not.
    ///
    /// Without both halves this could be a function that always returns
    /// `true` (making every gated test run and fail where tools are
    /// absent) or always `false` (silently skipping everything, forever).
    #[test]
    fn on_path_distinguishes_present_from_absent() {
        assert!(
            on_path("cargo"),
            "cargo must be on PATH for this test to be running at all"
        );
        assert!(!on_path("a-binary-that-does-not-exist-9f3a2b"));
    }

    /// `missing_icestorm_tools` agrees with `icestorm_available`.
    ///
    /// Cheap, and it catches the two drifting apart — a skip message
    /// listing nothing while the gate reports unavailable would be worse
    /// than no message.
    #[test]
    fn missing_list_agrees_with_the_gate() {
        assert_eq!(icestorm_available(), missing_icestorm_tools().is_empty());
    }
}
