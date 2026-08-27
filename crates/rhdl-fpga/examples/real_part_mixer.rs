// The output stage of a transmitter: a complex envelope modulated onto
// a complex carrier, keeping the real part.
//
// What to look for:
//
//   - `a` is the envelope and `b` is the carrier. Both are complex; the
//     output is `Real`. That asymmetry is the widget: it computes
//     `Re{a*b} = a.re*b.re - a.im*b.im`, which is two multiplies rather
//     than the four a full complex product needs, because `ad + bc` is
//     never formed.
//   - The carrier rotates at a quarter of the sample rate -- the
//     sequence (1,0), (0,1), (-1,0), (0,-1) scaled to full scale -- so
//     the output is the envelope's own rotation shifted up by fs/4.
//   - The envelope holds a constant complex value for the first stretch
//     and then rotates the other way. Watch the output: a constant
//     envelope gives a clean carrier, and a counter-rotating envelope
//     cancels the carrier's rotation and gives something much slower.
//     That is single-sideband behaviour, and it is why the *complex*
//     envelope matters -- a real envelope on a complex carrier can only
//     amplitude-modulate.
//   - `frame_mismatch` fires once, where the envelope claims a burst
//     start the carrier does not. In a real chain that means the two
//     have drifted and the transmitted phase is relative to an origin
//     the caller did not intend, so it is reported rather than resolved.
//   - `starved` fires where the envelope goes absent. The output for
//     that cycle is zero and un-marked: a cycle with no valid product
//     must not anchor anything.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::iq::Iq;
use rhdl_fpga::dsp::mixer::real_part::{In, RealPartMixer};
use rhdl_fpga::dsp::sync::SyncMark;
use rhdl_fpga::rcstream::Item;

const AW: usize = 8;
const BW: usize = 8;
const OW: usize = 9;
// A_W + B_W + 1: each output component is a difference of two products.
const PW: usize = 17;
const DROP: usize = 8;

/// Cycle the envelope starts counter-rotating on.
const ROTATE: usize = 12;

fn main() -> Result<(), RHDLError> {
    let uut = RealPartMixer::<SyncMark, AW, BW, OW, PW, DROP>::default();

    // A full-scale carrier at fs/4.
    let carrier = |n: usize| -> (i128, i128) {
        match n % 4 {
            0 => (100, 0),
            1 => (0, 100),
            2 => (-100, 0),
            _ => (0, -100),
        }
    };
    // A constant envelope, then one rotating the other way.
    let envelope = |n: usize| -> (i128, i128) {
        if n < ROTATE {
            (80, 0)
        } else {
            match n % 4 {
                0 => (80, 0),
                1 => (0, -80),
                2 => (-80, 0),
                _ => (0, 80),
            }
        }
    };

    let seq: Vec<In<SyncMark, AW, BW>> = (0..28)
        .map(|n| {
            let (er, ei) = envelope(n);
            let (cr, ci) = carrier(n);
            In::<SyncMark, AW, BW> {
                // Absent on one cycle, to show `starved`.
                a: if n == 20 {
                    None
                } else {
                    Some(Item::<Iq<AW>, SyncMark> {
                        data: Iq::<AW> {
                            re: signed::<AW>(er),
                            im: signed::<AW>(ei),
                        },
                        // Marked on the rotation boundary, and once more
                        // where the carrier disagrees.
                        frame: SyncMark {
                            sync: n == ROTATE || n == 24,
                        },
                    })
                },
                b: Some(Item::<Iq<BW>, SyncMark> {
                    data: Iq::<BW> {
                        re: signed::<BW>(cr),
                        im: signed::<BW>(ci),
                    },
                    frame: SyncMark { sync: n == ROTATE },
                }),
                downstream_ready: true,
            }
        })
        .collect();

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "real_part_mixer.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
