// The frequency terms of §8.3 composing into one `frequency_word`.
//
// A constant master frequency, a scheduled offset applied for a defined
// interval, a slow modulation ramp standing in for §8.6's eddy-current
// compensation input, and a constant calibration.
//
// What to look for:
//
//   - `frequency_word` rises when the scheduled offset is applied and
//     returns to the master rate when it is removed.
//   - Removing the offset restores the *slope*. It does not erase phase
//     already accumulated -- that is the distinction §8.3 warns must
//     not be confused, and it lives in the accumulator, not here.
//     Returning to the unmodulated trajectory instead needs a
//     compensating phase term through the phase composer.
//   - Output lags the terms by one cycle, which is
//     `latency::FREQUENCY_COMPOSER`.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    dsp::nco::frequency_composer::{FrequencyComposer, In},
};

const W: usize = 16;

fn main() -> Result<(), RHDLError> {
    let uut = FrequencyComposer::<W>::default();

    let stream = (0..40u128)
        .map(|k| In::<W> {
            master: bits::<W>(8000),
            scheduled_offset: bits::<W>(if (14..26).contains(&k) { 2000 } else { 0 }),
            modulation: bits::<W>(k * 20),
            calibration: bits::<W>(50),
        })
        .with_reset(1)
        .clock_pos_edge(100);

    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "nco_frequency_composer.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
