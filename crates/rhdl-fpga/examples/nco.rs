// The complete NCO: a commanded frequency in Hz, and a phase step
// applied and removed.
//
// What to look for:
//
//   - `sin` and `cos` are a quarter cycle apart -- one quarter-wave
//     table read at two addresses.
//   - Partway through, a half-turn phase offset is applied and then
//     removed. The output jumps and comes back; `master` never
//     flinches. That is the property the whole design rests on: an
//     experiment repeated after an arbitrary delay sees the phase the
//     free-running oscillator would have had.
//   - `master` advances at the commanded rate throughout, 48 bits wide.
//     Only its top 22 bits reach phase-to-amplitude -- the truncation
//     that the spur analysis is about.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::nco::{
    composite::{In, Nco},
    config::{self, PHASE_W},
    frequency_composer, phase_composer,
};

fn main() -> Result<(), RHDLError> {
    let uut = Nco::default();

    // 125 MHz / 64 = 1.953125 MHz, so one output cycle per 64 samples.
    let hz = config::F_SAMPLE_HZ as u128 / 64;
    let word = config::tuning_word(hz * 1_000_000);

    let stream = (0..160u128)
        .map(|k| In {
            frequency: frequency_composer::In::<PHASE_W> {
                master: bits::<PHASE_W>(word),
                scheduled_offset: bits::<PHASE_W>(0),
                modulation: bits::<PHASE_W>(0),
                calibration: bits::<PHASE_W>(0),
            },
            phase: phase_composer::In::<PHASE_W> {
                // A half turn, applied over a window and then removed.
                pulse: bits::<PHASE_W>(if (64..112).contains(&k) {
                    config::phase_word(180_000)
                } else {
                    0
                }),
                frame: bits::<PHASE_W>(0),
                calibration: bits::<PHASE_W>(0),
                fine_time: bits::<PHASE_W>(0),
                trim: bits::<PHASE_W>(0),
            },
        })
        .with_reset(1)
        .clock_pos_edge(100);

    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(vcd, "nco.md", SvgOptions::default().with_io_filter())?;
    Ok(())
}
