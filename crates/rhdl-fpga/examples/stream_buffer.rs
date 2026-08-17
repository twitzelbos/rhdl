use rhdl::prelude::*;
use rhdl_fpga::doc::DetRng;
use rhdl_fpga::{
    rng::xorshift::XorShift128,
    stream::stream_buffer::{self, StreamBuffer},
};

fn main() -> Result<(), RHDLError> {
    // The buffer will manage items of 4 bits
    let uut = StreamBuffer::<b4>::default();
    // Create a random data stream that pauses randomly
    let mut source_rng = XorShift128::default();
    let mut det_src = DetRng::new(0x1000);
    let mut send = (0..).map(move |_| {
        if det_src.chance(78) {
            source_rng.next().map(|x| bits((x & 0xF) as u128))
        } else {
            None
        }
    });
    let mut det_sink = DetRng::new(0x3711);
    let mut need_reset = true;
    let vcd = uut
        .run_fn(
            |out| {
                if need_reset {
                    need_reset = false;
                    return Some(rhdl::core::sim::ResetOrData::Reset);
                }
                let mut input = stream_buffer::In::<b4>::dont_care();
                // if the valid flag is high, then advance the source RNG (maybe)
                input.data = None;
                if out.ready.raw {
                    input.data = send.next().unwrap();
                }
                input.ready.raw = det_sink.chance(67);
                Some(rhdl::core::sim::ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 1500)
        .collect::<SvgFile>();
    rhdl_fpga::doc::write_svg_as_markdown(
        vcd,
        "option_carloni.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
