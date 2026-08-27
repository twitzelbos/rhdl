// A digital up-converter with a quadrature output: the same chain as
// `duc_real`, emitting both components.
//
// Use this one when the passband is formed outside the FPGA -- a
// quadrature modulator, an I/Q DAC pair, a transceiver taking baseband I
// and Q. If a single DAC carries the signal, `RealDuc` is the same chain
// with two multiplies in the mixer instead of four.
//
// What to look for:
//
//   - `stream.data` carries `re` and `im`, where `duc_real`'s carries one
//     value. Both quadratures are present on every cycle.
//   - The two components are in quadrature: `im` lags `re` by a quarter
//     period of the carrier. That is the signature of a
//     frequency-translated *complex* baseband, and it is not the same
//     thing as two independent real signals -- a real passband signal
//     has conjugate-symmetric spectrum and this does not. A downstream
//     stage that forgets the difference will get the sideband arithmetic
//     wrong.
//   - `re` here is bit-for-bit what `duc_real` emits from the same input.
//     The two chains compute it by different routes -- four products
//     versus two -- and a test in `duc/iq.rs` requires them to agree,
//     which is what validates both mixers against each other.
//   - The envelope rotates one quarter turn per envelope sample, so the
//     signal sits above the carrier and nothing sits below it.
//   - `stream.ready` pulses once every eight cycles, and `master` climbs
//     by the tuning word, exactly as in `duc_real`. The pull contract and
//     the phase reference are properties of the shared front end and the
//     shared oscillator, not of the mixer.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::cic::interpolator::CicInterpolate;
use rhdl_fpga::dsp::duc::iq::{In, IqDuc};
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
    let uut = IqDuc::<W, WA, CW, OW, PROD_W, DROP, Core>::default();

    let frequency = bits::<PHASE_W>(1u128 << (PHASE_W - 2));

    let seq: Vec<In<W, CW>> = (0..8 * RATE)
        .map(|n| {
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
    write_svg_as_markdown(vcd, "duc_iq.md", SvgOptions::default().with_io_filter())?;
    Ok(())
}
