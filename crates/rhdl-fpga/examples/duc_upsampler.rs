// A complex envelope brought up to the converter rate, interpolating by
// four.
//
// What to look for:
//
//   - `stream.ready` pulses once every four cycles. This is the chain
//     asking for its next envelope sample, and it is what makes an
//     up-converter a *pull* rather than a push: whatever generates the
//     envelope has to answer this, where a down-converter's input just
//     arrives.
//   - `stream.data` is present on every cycle, carrying both
//     quadratures at the converter rate.
//   - The envelope steps from (60, -20) to (-40, 50) partway through,
//     marked on its first sample. Watch both quadratures move together
//     and take a whole window to get there: two CIC stages is linear
//     interpolation, so a step at the low rate becomes a ramp at the
//     high one. Neither arm leads the other -- if they did, the
//     constellation would rotate during the transition and energy would
//     leak into the sideband the modulation was meant to suppress.
//   - The mark restarts both arms at once, so the output drops to zero
//     and refills. Both arms are the same type at the same
//     configuration, which is what makes "at once" a property of the
//     construction rather than a hope.
//   - `frame_mismatch` stays low. It fires only if the two arms
//     disagreed about framing, which cannot happen while they are fed
//     from one split -- so if it ever fires, they have drifted.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::cic::interpolator::CicInterpolate;
use rhdl_fpga::dsp::duc::upsampler::{EnvelopeUpsampler, In};
use rhdl_fpga::dsp::iq::Iq;
use rhdl_fpga::dsp::sync::SyncMark;
use rhdl_fpga::rcstream::Item;

const W: usize = 8;
const WA: usize = 11;
const S: usize = 2;
const RMAX: usize = 8;
const M: usize = 1;
const CW: usize = 4;
const RATE: usize = 4;

/// Cycle the second, marked, envelope begins on.
const SWITCH: usize = 24;

type Core = CicInterpolate<W, WA, S, RMAX, M, CW>;

fn main() -> Result<(), RHDLError> {
    let uut = EnvelopeUpsampler::<W, WA, CW, Core>::default();

    let seq: Vec<In<W, CW>> = (0..SWITCH + 28)
        .map(|n| {
            let (re, im) = if n < SWITCH { (60, -20) } else { (-40, 50) };
            In::<W, CW> {
                stream: Some(Item::<Iq<W>, SyncMark> {
                    data: Iq::<W> {
                        re: signed::<W>(re),
                        im: signed::<W>(im),
                    },
                    // Held for a window, the way a real upstream holds a
                    // sample until `ready`. Consumed once.
                    frame: SyncMark {
                        sync: (SWITCH..SWITCH + RATE).contains(&n),
                    },
                }),
                rate: bits::<CW>(RATE as u128),
                downstream_ready: true,
            }
        })
        .collect();

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "duc_upsampler.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
