// A digital up-converter for a single DAC: a complex envelope modulated
// onto a coherent carrier, real output.
//
// The configuration here is small enough to read in a waveform --
// interpolating by eight, an 8-bit envelope, a carrier at a quarter of
// the sample rate. `RealDuc`'s module docs carry the sizing for the case
// this widget was written for: a 1 Msps complex envelope onto a 125 Msps
// carrier, which is R = 125.
//
// What to look for:
//
//   - `stream.ready` pulses once every eight cycles: the chain asking
//     for its next envelope sample. An up-converter pulls.
//   - `stream.data` is present on every cycle. That is what a DAC wants,
//     and it is why `downstream_ready` should be tied high in a real
//     design -- a low cycle loses that sample and `overrun` says so.
//   - The envelope rotates one quarter turn per envelope sample. Because
//     the modulation is a true complex multiply, this puts the signal
//     *above* the carrier only; the mirror image below it is absent. A
//     real envelope could not do that -- its two sidebands would be
//     forced to be mirror images. That is the whole reason the envelope
//     is complex, and reversing the rotation moves the signal to the
//     other side.
//   - The output is a clean tone, not a staircase. The interpolator's
//     sinc^2 nulls sit exactly on the images the eight-fold upsampling
//     creates, so what reaches the DAC has had them removed rather than
//     merely attenuated.
//   - `master` climbs steadily by the tuning word. It is absolute
//     elapsed phase and is never reset at a burst boundary, which is
//     what lets a receiver relate its measurement to this transmission.
//   - The first envelope sample is marked, so the burst has an origin
//     the far end can be told about. The mark rides out on the first
//     output of the window.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::cic::interpolator::CicInterpolate;
use rhdl_fpga::dsp::duc::real::{In, RealDuc};
use rhdl_fpga::dsp::iq::Iq;
use rhdl_fpga::dsp::nco::config::PHASE_W;
use rhdl_fpga::dsp::sync::SyncMark;
use rhdl_fpga::rcstream::Item;

const W: usize = 8;
const WA: usize = 11;
const S: usize = 2;
const RMAX: usize = 8;
const M: usize = 1;
const CW: usize = 4;
const OW: usize = 12;
const PROD_W: usize = WA + 18 + 1;
const DROP: usize = PROD_W - OW;
const RATE: usize = 8;

type Core = CicInterpolate<W, WA, S, RMAX, M, CW>;

fn main() -> Result<(), RHDLError> {
    let uut = RealDuc::<W, WA, CW, OW, PROD_W, DROP, Core>::default();

    // A carrier at a quarter of the sample rate, exactly.
    let frequency = bits::<PHASE_W>(1u128 << (PHASE_W - 2));

    let seq: Vec<In<W, CW>> = (0..8 * RATE)
        .map(|n| {
            // One quarter turn per envelope sample.
            let (re, im) = match (n / RATE) % 4 {
                0 => (90, 0),
                1 => (0, 90),
                2 => (-90, 0),
                _ => (0, -90),
            };
            In::<W, CW> {
                stream: Some(Item::<Iq<W>, SyncMark> {
                    data: Iq::<W> {
                        re: signed::<W>(re),
                        im: signed::<W>(im),
                    },
                    frame: SyncMark { sync: n == 0 },
                }),
                rate: bits::<CW>(RATE as u128),
                frequency,
                phase: bits::<PHASE_W>(0),
                downstream_ready: true,
            }
        })
        .collect();

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(vcd, "duc_real.md", SvgOptions::default().with_io_filter())?;
    Ok(())
}
