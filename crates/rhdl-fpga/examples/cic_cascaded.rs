// Two framed decimators in series: /2 then /4, so /8 overall.
//
// What to look for:
//
//   - Output items appear once every eight input samples. Neither
//     stage's rate is visible from outside: the cascade presents the
//     same framed-decimator interface either stage does, so it looks
//     like a single /8 decimator to anything downstream.
//   - The input is a constant 100 at first, and the output settles at
//     100 * 2^2 * 4^2 = 6400. Each stage contributes its own (R*M)^N
//     and the cascade contributes the product -- that settled value is
//     how you confirm both stages are really in series.
//   - Then one input carries `sync: true`, deliberately placed off the
//     composite grid. It comes out the far end, on one output item,
//     once. Between them the two stages discard seven of every eight
//     frames, and the mark survives because each stage latches its own.
//   - **Nothing in the cascade arranges that.** The second stage
//     restarts because its own input is marked; it has no idea there is
//     a stage in front of it. The cascade's kernel is three
//     assignments and holds no state.
//   - Then a tone at the first stage's null. The output collapses --
//     the first stage rejects it before the second stage ever sees it,
//     which is why the cheap fast stage goes first.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::cic::cascaded::CascadedDecimator;
use rhdl_fpga::dsp::cic::stream::{In, StreamDecimator};
use rhdl_fpga::dsp::cic::{CicDecimate, accumulator_width, counter_width};
use rhdl_fpga::dsp::iq::Real;
use rhdl_fpga::dsp::sync::SyncMark;
use rhdl_fpga::rcstream::Item;

const WI: usize = 10;
const R1: usize = 2;
const R2: usize = 4;
const N1: usize = 2;
const N2: usize = 2;
const M: usize = 1;
const WMID: usize = accumulator_width(WI, N1, R1, M);
const WOUT: usize = accumulator_width(WMID, N2, R2, M);

type First = StreamDecimator<WI, WMID, CicDecimate<WI, WMID, N1, R1, M, { counter_width(R1) }>>;
type Second =
    StreamDecimator<WMID, WOUT, CicDecimate<WMID, WOUT, N2, R2, M, { counter_width(R2) }>>;
type Uut = CascadedDecimator<WI, WMID, WOUT, First, Second>;

fn main() -> Result<(), RHDLError> {
    let uut = Uut::default();

    let item = |v: i128, sync: bool| In::<WI> {
        stream: Some(Item::<Real<WI>, SyncMark> {
            data: Real::<WI> { v: signed::<WI>(v) },
            frame: SyncMark { sync },
        }),
        downstream_ready: true,
    };
    let idle = In::<WI> {
        stream: None,
        downstream_ready: true,
    };

    let mut seq: Vec<In<WI>> = Vec::new();
    // Settle at DC.
    for _ in 0..32 {
        seq.push(item(100, false));
    }
    // A mark off the composite grid: 5 is not a multiple of 8.
    seq.push(item(100, true));
    for _ in 0..23 {
        seq.push(item(100, false));
    }
    // A tone at the first stage's null, fs/2.
    for k in 0..24 {
        seq.push(item(if k % 2 == 0 { 100 } else { -100 }, false));
    }
    seq.push(idle);

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "cic_cascaded.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
