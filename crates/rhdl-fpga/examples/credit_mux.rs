// Aggregate three credit-based sources into one RCStream.
//
// Each source has its own credit pool and buffer, so one source cannot
// starve another; a round-robin arbiter merges them. Watch the three
// credit_grant wires move independently while the merged data stream
// interleaves the sources fairly.
//
// Deterministic (no RNG).

use rhdl::{core::sim::ResetOrData, prelude::*};
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    rcstream::{
        bus::Item,
        credit::mux::{CreditMux, In},
    },
};

type Mux = CreditMux<b8, (), 5, 2, 2, 3>;

fn main() -> Result<(), RHDLError> {
    let uut = Mux::default();
    let mut sent = [0u128; 3];
    let mut credit = [0u128; 3];
    let mut need_reset = true;
    let mut phase: u32 = 0;

    let vcd = uut
        .run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase = phase.wrapping_add(1);
                let mut input = In::<b8, (), 3> {
                    data: [None; 3],
                    // Periodic backpressure, so the arbiter has to hold.
                    ready: !phase.is_multiple_of(3),
                };
                // Each source keeps a real credit counter: grants
                // accumulate, a send decrements.  Gating on the
                // instantaneous grant instead would overrun the sink.
                for k in 0..3usize {
                    credit[k] += output.credit_grant[k].raw();
                    if credit[k] > 0 {
                        input.data[k] = Some(Item::<b8, ()> {
                            data: b8(((k as u128) * 100 + sent[k]) % 256),
                            frame: (),
                        });
                        sent[k] += 1;
                        credit[k] -= 1;
                    }
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 1500)
        .collect::<SvgFile>();

    write_svg_as_markdown(vcd, "credit_mux.md", SvgOptions::default().with_io_filter())?;
    Ok(())
}
