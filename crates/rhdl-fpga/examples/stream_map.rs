use std::iter::repeat_n;

use rhdl::prelude::*;
use rhdl_fpga::doc::DetRng;
use rhdl_fpga::stream::testing::sink_from_fn::SinkView;
use rhdl_fpga::{
    core::slice::lsbs,
    rng::xorshift::XorShift128,
    stream::{
        map::Map,
        testing::{single_stage::single_stage, utils::stalling},
    },
};

#[kernel]
fn map_item(_cr: ClockReset, t: b4) -> b2 {
    lsbs::<2, 4>(t)
}

fn main() -> Result<(), RHDLError> {
    let a_rng = XorShift128::default().map(|x| b4((x & 0xF) as u128));
    let mut b_rng = a_rng.clone();
    let a_rng = stalling(a_rng, 0.23);
    let mut det = DetRng::new(0x1000);
    let consume = move |v: SinkView<b2>| {
        if let Some(data) = v.accepted {
            let orig = b_rng.next().unwrap();
            let orig_lsb = lsbs::<2, 4>(orig);
            assert_eq!(data, orig_lsb);
        }
        det.chance(80)
    };
    let map = Map::try_new::<map_item>()?;
    let uut = single_stage(map, a_rng, consume);
    // Run a few samples through
    let input = repeat_n((), 15).with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(input).collect::<SvgFile>();
    rhdl_fpga::doc::write_svg_as_markdown(vcd, "stream_map.md", SvgOptions::default())?;
    Ok(())
}
