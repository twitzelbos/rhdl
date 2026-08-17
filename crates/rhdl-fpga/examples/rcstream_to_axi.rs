// A typed `RCStream` driven out as an AXI4-Stream master.
//
// The widget unpacks `Option<Item<T, F>>` into `(TDATA, TUSER, TVALID)`
// and takes `TREADY` from the bus, with a Carloni skid buffer isolating
// the AXI side.
//
// The interesting obligation is AMBA's: once `TVALID` is asserted, the
// beat must be held stable until `TREADY` is seen, and must not be
// dropped or replaced.  `TREADY` is deasserted on one cycle in three
// here so the trace actually shows a held beat; against a
// permanently-ready consumer that rule is never tested.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    rcstream::{axi_stream::rcstream_to_axi::RCStreamToAxi, bus::Item},
};

fn main() -> Result<(), RHDLError> {
    let uut: RCStreamToAxi<b8, ()> = RCStreamToAxi::default();

    let stream = (0..24u128)
        .map(|k| rhdl_fpga::rcstream::axi_stream::rcstream_to_axi::In {
            data: if k.is_multiple_of(4) {
                None
            } else {
                Some(Item::<b8, ()> {
                    data: b8(k % 256),
                    frame: (),
                })
            },
            tready: !k.is_multiple_of(3),
        })
        .with_reset(1)
        .clock_pos_edge(100);

    let vcd = uut
        .run(stream)
        .take_while(|t| t.time < 1500)
        .collect::<SvgFile>();

    write_svg_as_markdown(
        vcd,
        "rcstream_to_axi.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
