// A scheduled frequency segment: a linear chirp, loaded once and then
// advancing every sample with no further control traffic.
//
// What to look for:
//
//   - `load` is a single cycle. Everything after it is the hardware
//     advancing on its own -- that is the point of §8.5: the scheduler
//     describes a segment, it does not drive the sweep.
//   - `word` ramps linearly, then `done` pulses and `running` drops.
//   - The final `word` is the commanded endpoint **exactly**. On the
//     last sample the accumulator is loaded with `end_word` rather than
//     stepped to it, so rounding in `step` cannot leave the segment
//     ending at an almost-right frequency.
//
// The step here is deliberately fractional: the accumulator carries 16
// bits below the frequency word, because a slow ramp's per-sample step
// is *less* than one LSB and an integer accumulator would emit a flat
// line. See the module docs.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::nco::config::PHASE_W;
use rhdl_fpga::dsp::nco::ramp::{ACC_W, CNT_W, FrequencyRamp, In};

fn main() -> Result<(), RHDLError> {
    let uut = FrequencyRamp::default();

    const SAMPLES: u128 = 24;
    let start = 1_000_000u128;
    let end = 1_000_600u128;
    // (end - start) << FRAC_W / SAMPLES, computed here rather than in
    // hardware -- division belongs to the scheduler.
    let step = ((end - start) << 16) / SAMPLES;

    let idle = In {
        load: false,
        start_word: bits::<PHASE_W>(0),
        end_word: bits::<PHASE_W>(0),
        step: bits::<ACC_W>(0),
        samples: bits::<CNT_W>(0),
    };

    let mut seq = vec![In {
        load: true,
        start_word: bits::<PHASE_W>(start),
        end_word: bits::<PHASE_W>(end),
        step: bits::<ACC_W>(step),
        samples: bits::<CNT_W>(SAMPLES),
    }];
    seq.extend(std::iter::repeat_n(idle, 32));

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(vcd, "nco_ramp.md", SvgOptions::default().with_io_filter())?;
    Ok(())
}
