// An AXI4-Stream master input translated into a typed `RCStream`.
//
// The widget packs `(TDATA, TUSER, TVALID)` into `Option<Item<T, F>>`
// and presents `TREADY` back to the bus.  A Carloni skid buffer sits in
// between, so the AXI side is isolated from combinational paths in
// whatever consumes the `RCStream`.
//
// `TVALID` is gapped on one cycle in four and the downstream withholds
// `ready` on one in three.  The two cadences are deliberately coprime so
// they drift against each other and the trace covers all four
// combinations of (offer, accept) rather than just the aligned ones.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::{doc::write_svg_as_markdown, rcstream::axi_stream::axi_to_rcstream::AxiToRCStream};

fn main() -> Result<(), RHDLError> {
    let uut: AxiToRCStream<b8, ()> = AxiToRCStream::default();

    let stream = (0..24u128)
        .map(|k| rhdl_fpga::rcstream::axi_stream::axi_to_rcstream::In {
            tdata: b8(k % 256),
            tuser: (),
            tvalid: !k.is_multiple_of(4),
            ready: !k.is_multiple_of(3),
        })
        .with_reset(1)
        .clock_pos_edge(100);

    let vcd = uut
        .run(stream)
        .take_while(|t| t.time < 1500)
        .collect::<SvgFile>();

    write_svg_as_markdown(
        vcd,
        "axi_to_rcstream.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
