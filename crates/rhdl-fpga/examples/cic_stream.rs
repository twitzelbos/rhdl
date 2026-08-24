// A decimator on a framed stream, showing what happens to the mark.
//
// A two-stage CIC decimating by four, wrapped so it speaks in framed
// `Item`s rather than bare samples.
//
// What to look for:
//
//   - Output items appear once every four input items. That is the
//     decimation, unchanged by the framing.
//   - The input carries `sync: true` on exactly one sample, and it is
//     deliberately *not* one of the samples that survives decimation.
//     The mark still comes out -- on the next output item -- because it
//     is latched across the window. Three of every four marks would be
//     lost if the output simply took the surviving sample's frame.
//   - That marked output is also the first of a fresh window: the mark
//     restarts the decimator, so the sample carrying it is built only
//     from post-trigger inputs. The mark names a boundary rather than
//     pointing near one.
//   - Watch which output gets the mark. It is not the one emerging on
//     the same cycle the mark arrives -- that one was registered from
//     the previous cycle and belongs entirely to the old window. Only a
//     mark carried over from an earlier cycle rides out.
//   - `sample: None` cycles hold the filter: its state is over samples,
//     not cycles.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::cic::stream::{In, StreamDecimator};
use rhdl_fpga::dsp::cic::{CicDecimate, accumulator_width, counter_width};
use rhdl_fpga::dsp::iq::Real;
use rhdl_fpga::dsp::sync::SyncMark;
use rhdl_fpga::rcstream::Item;

const WI: usize = 12;
const N: usize = 2;
const R: usize = 4;
const M: usize = 1;
const WA: usize = accumulator_width(WI, N, R, M);
const CW: usize = counter_width(R);

type Cic = CicDecimate<WI, WA, N, R, M, CW>;
type Uut = StreamDecimator<WI, WA, Cic>;

fn main() -> Result<(), RHDLError> {
    let uut = Uut::default();

    let item = |v: i128, sync: bool| In::<WI> {
        stream: Some(Item::<Real<WI>, SyncMark> {
            data: Real::<WI> { v: signed::<WI>(v) },
            frame: SyncMark { sync },
        }),
        restart: false,
        downstream_ready: true,
    };
    let idle = In::<WI> {
        stream: None,
        restart: false,
        downstream_ready: true,
    };

    let mut seq: Vec<In<WI>> = Vec::new();
    // A settled run-in at a constant level.
    for _ in 0..12 {
        seq.push(item(100, false));
    }
    // The mark lands mid-window, on a sample decimation discards.
    for k in 0..12 {
        seq.push(item(100, k == 1));
    }
    // A gap, to show the filter holding.
    seq.push(idle);
    seq.push(idle);
    for _ in 0..8 {
        seq.push(item(100, false));
    }
    seq.push(idle);

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(vcd, "cic_stream.md", SvgOptions::default().with_io_filter())?;
    Ok(())
}
