// A `CreditSource` running out of credit and recovering.
//
// The source may only send while its local credit counter is non-zero.
// The trace shows the counter being spent down to empty, `upstream_ready`
// dropping as a result, and sending resuming as grants arrive.
//
// The send decision reads the *latched* counter, not the grant arriving
// this cycle.  That is the point of credit-based flow control: it breaks
// the long sink-to-source combinational path that a plain Ready/Valid
// handshake has, at the cost of a round trip's worth of latency.
//
// Credit is granted on one cycle in four, so the source genuinely
// starves.  Granting every cycle would keep the counter saturated and
// the can't-send path — the reason the widget exists — would never
// appear in the trace.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    rcstream::{bus::Item, credit::source::CreditSource},
};

const CW: usize = 4;

fn main() -> Result<(), RHDLError> {
    let uut: CreditSource<b8, (), CW> = CreditSource::default();

    let stream = (0..28u128)
        .map(|k| rhdl_fpga::rcstream::credit::source::In {
            upstream_data: Some(Item::<b8, ()> {
                data: b8(k % 256),
                frame: (),
            }),
            credit_grant: bits::<CW>(u128::from(k.is_multiple_of(4))),
        })
        .with_reset(1)
        .clock_pos_edge(100);

    let vcd = uut
        .run(stream)
        .take_while(|t| t.time < 1500)
        .collect::<SvgFile>();

    write_svg_as_markdown(
        vcd,
        "credit_source.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
