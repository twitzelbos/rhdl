// Recombine two real streams into one complex stream, and check that
// their framing markers agree.
//
// What to look for:
//
//   - `stream` carries an `Iq` built from the two halves, marked with
//     the framing the two sides agreed on.
//   - `frame_mismatch` fires on exactly the cycle where the two sides
//     disagree about the marker. That is the whole reason this widget
//     validates rather than just moves: two streams split from one
//     source should carry identical markers, so a disagreement means
//     something upstream desynchronised them. Reported, not resolved --
//     an earlier version took the real side's frame and discarded the
//     imaginary side's, which turned drift into a confident wrong
//     answer.
//   - A one-sided cycle -- one half present, the other absent --
//     produces no output. Half a complex value is not a complex value.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::{core::sim::ResetOrData, prelude::*};
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    dsp::{
        iq::{Imag, Real},
        sync::SyncMark,
    },
    rcstream::{
        bus::Item,
        util::combine::{In, IqCombine},
    },
};

const W: usize = 12;

/// Cycle on which the two sides disagree about the marker.
const DISAGREE: u128 = 8;
/// Cycle on which only the real half is present.
const ONE_SIDED: u128 = 12;

fn main() -> Result<(), RHDLError> {
    let uut = IqCombine::<W, SyncMark>::default();
    let mut n: u128 = 0;
    let mut need_reset = true;

    let vcd = uut
        .run_fn(
            |_output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                let c = n;
                n += 1;
                let k = c as i128;
                // Both sides marked together on cycle 4; only the real
                // side on `DISAGREE`, which is what must be flagged.
                let re_mark = c == 4 || c == DISAGREE;
                let im_mark = c == 4;
                Some(ResetOrData::Data(In::<W, SyncMark> {
                    real: Some(Item::<Real<W>, SyncMark> {
                        data: Real::<W> {
                            v: signed::<W>(90 * k),
                        },
                        frame: SyncMark { sync: re_mark },
                    }),
                    imag: if c == ONE_SIDED {
                        None
                    } else {
                        Some(Item::<Imag<W>, SyncMark> {
                            data: Imag::<W> {
                                v: signed::<W>(-60 * k),
                            },
                            frame: SyncMark { sync: im_mark },
                        })
                    },
                    downstream_ready: true,
                }))
            },
            100,
        )
        .take_while(|t| t.time < 2000)
        .collect::<SvgFile>();

    write_svg_as_markdown(vcd, "iq_combine.md", SvgOptions::default().with_io_filter())?;
    Ok(())
}
