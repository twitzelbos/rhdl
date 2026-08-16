// Move an `RCStream` of `b8` items from the `Red` clock domain (period
// 50) to the `Blue` clock domain (period 78).
//
// The source is deliberately aggressive: it presents an item on every
// cycle, including cycles where the crossing has deasserted `ready`.
// That is legal under the bus contract, and it is what the crossing's
// write gate exists to absorb.  The sink applies periodic backpressure
// so the internal FIFO actually fills and `ready` drops.
//
// Everything here is deterministic — no RNG — so the committed trace
// regenerates byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    rcstream::{bus::Item, cdc::RCStreamCdc},
};

fn main() -> Result<(), RHDLError> {
    let uut = RCStreamCdc::<b8, (), Red, Blue, 3>::default();

    // Source state: the next payload value to present.
    let mut next_to_send: u128 = 0;
    // Sink state: a fixed period-3 backpressure pattern.
    let mut phase: u32 = 0;

    let vcd = run_async_red_blue(
        &uut,
        // Red (W) — the source.  Always presenting; advances only when
        // the crossing signalled that it had room.
        |output, input| {
            input.data = signal(Some(Item::<b8, ()> {
                data: b8(next_to_send % 256),
                frame: (),
            }));
            if output.ready.val() {
                next_to_send += 1;
            }
        },
        // Blue (R) — the sink.  Accepts on 2 of every 3 cycles.
        |output, input| {
            phase = phase.wrapping_add(1);
            let want = !phase.is_multiple_of(3);
            input.ready = signal(want && output.data.val().is_some());
        },
        50,
        78,
        |red, blue, input| {
            input.cr_w = red;
            input.cr_r = blue;
        },
    )
    .take_while(|t| t.time < 1500)
    .collect::<SvgFile>();

    let options = SvgOptions::default().with_io_filter();
    write_svg_as_markdown(vcd, "rcstream_cdc.md", options)?;
    Ok(())
}
