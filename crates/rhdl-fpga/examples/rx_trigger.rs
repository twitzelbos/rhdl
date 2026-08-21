// Marking the start of an acquisition, at the source.
//
// What to look for:
//
//   - `arm` pulses on sample 8, against a live sample. The `sync`
//     marker appears on the output one cycle later --
//     RX_TRIGGER_LATENCY -- riding with the very sample that was
//     present when the pulse arrived.
//   - One marked sample per pulse. The request is consumed by the
//     sample that takes it, so nothing downstream sees an ambiguous
//     anchor. `arm` is also rising-edge sensitive, so holding the line
//     high would still request just one.
//   - The second pulse, on sample 20, lands in a deliberate gap where
//     `stream` is None. It is not dropped: `armed` goes high and stays
//     high until sample 23, the first live sample after the gap, which
//     takes the mark. That is the difference between a latched request
//     and a level-sensitive one.
//   - `armed` is low everywhere else. With an isochronous source a
//     sample is always there to consume the request in the same cycle,
//     so the pending state is normally invisible from outside -- the
//     gap is here to make it observable.
//   - The payload is untouched. A trigger names a sample; it does not
//     alter one.
//
// The two marked positions are pinned by
// `the_example_behaves_as_its_comments_claim` in the module's tests, so
// this description cannot drift away from the widget.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::iq::Iq;
use rhdl_fpga::dsp::rx_trigger::{In, RxTrigger};
use rhdl_fpga::rcstream::bus::Item;

const W: usize = 18;

fn main() -> Result<(), RHDLError> {
    let uut = RxTrigger::<W>::default();

    // A slowly rotating carrier so the payload is visibly untouched,
    // with a gap in the stream around sample 20 to show `armed`.
    let n = 28i128;
    let seq: Vec<In<W>> = (0..n)
        .map(|k| {
            let theta = 2.0 * std::f64::consts::PI * (k as f64) / 16.0;
            let amp = 100_000.0;
            let idle = (20..23).contains(&k);
            In::<W> {
                stream: if idle {
                    None
                } else {
                    Some(Item::<Iq<W>, ()> {
                        data: Iq::<W> {
                            re: signed::<W>((theta.cos() * amp) as i128),
                            im: signed::<W>((theta.sin() * amp) as i128),
                        },
                        frame: (),
                    })
                },
                // One pulse against a live sample, one against a gap.
                arm: k == 8 || k == 20,
                downstream_ready: true,
            }
        })
        .collect();

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(vcd, "rx_trigger.md", SvgOptions::default().with_io_filter())?;
    Ok(())
}
