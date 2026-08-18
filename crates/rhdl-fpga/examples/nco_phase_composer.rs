// The layered phase terms of §8.2 composing into one `phase_offset`.
//
// Three terms move independently: a pulse phase that steps through
// quadrants, a frame phase applied over a window and then removed, and
// a constant channel calibration. The output is their sum modulo one
// full turn.
//
// What to look for:
//
//   - `phase_offset` is the running sum. When `frame` returns to zero
//     the offset returns to exactly what pulse + calibration give,
//     with no residue -- the terms compose and decompose cleanly.
//   - The sum wraps rather than saturating. A full turn is
//     indistinguishable from none, which is the arithmetic, not an
//     overflow.
//   - Output lags the terms by one cycle: the sum is registered, which
//     is `latency::PHASE_COMPOSER`.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    dsp::nco::phase_composer::{In, PhaseComposer},
};

const W: usize = 16;

fn main() -> Result<(), RHDLError> {
    let uut = PhaseComposer::<W>::default();

    let stream = (0..40u128)
        .map(|k| In::<W> {
            // A quarter turn every eight cycles, wrapping after a full turn:
            // 4 * 16384 would be 2^16, one past what Bits<16> holds.
            pulse: bits::<W>(((k / 8) % 4) * 16384),
            // Applied over a window, then removed.
            frame: bits::<W>(if (12..28).contains(&k) { 4096 } else { 0 }),
            calibration: bits::<W>(300),
            fine_time: bits::<W>(0),
            trim: bits::<W>(0),
        })
        .with_reset(1)
        .clock_pos_edge(100);

    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "nco_phase_composer.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
