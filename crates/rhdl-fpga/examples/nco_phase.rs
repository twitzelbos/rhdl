// A free-running master phase accumulator with a phase offset applied
// and then removed.
//
// The trace shows the property the widget exists for: while the offset
// is applied, `phase` steps away from `master`; when the offset returns
// to zero, `phase` rejoins the trajectory `master` has been following
// all along, undisturbed. Nothing about the master accumulator records
// that an offset ever happened.
//
// That is what makes phase coherent across pulses: an experiment
// repeated after an arbitrary delay sees the phase the free-running
// oscillator would have had, not one that depends on when the last
// pulse ended.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::{doc::write_svg_as_markdown, dsp::nco::phase_accumulator::PhaseAccumulator};

const W: usize = 16;

fn main() -> Result<(), RHDLError> {
    let uut = PhaseAccumulator::<W>::default();

    // A frequency word chosen so the accumulator wraps within the
    // window, and an offset applied over a visible span.
    let stream = (0..48u128)
        .map(|k| rhdl_fpga::dsp::nco::phase_accumulator::In::<W> {
            frequency_word: bits::<W>(2048),
            phase_offset: if (16..32).contains(&k) {
                bits::<W>(16384) // a quarter turn
            } else {
                bits::<W>(0)
            },
        })
        .with_reset(1)
        .clock_pos_edge(100);

    let vcd = uut
        .run(stream)
        .take_while(|t| t.time < 1500)
        .collect::<SvgFile>();

    write_svg_as_markdown(vcd, "nco_phase.md", SvgOptions::default().with_io_filter())?;
    Ok(())
}
