//! In this test fixture, we assemble a source, a flatten, and a sink
//! into a single assembly:
//!
#![doc=badascii!(r"
+-+Source+-+     +-+UUT+------+      +-+Sink+---+
|          | ?T  |            | ?S   |          |
|     data +---->|in       out+----->|data      |
|          |     |            |      |          |
|          |     |            |      |          |
|    ready |<---+|ready  ready|<-----+ready     |
+----------+     +------------+      +----------+
")]

use badascii_doc::badascii;
use rhdl::prelude::*;
use rhdl_fpga::doc::DetRng;
use rhdl_fpga::stream::testing::sink_from_fn::SinkView;
use rhdl_fpga::{
    rng::xorshift::XorShift128,
    stream::{
        flatten::Flatten,
        testing::{single_stage::single_stage, utils::stalling},
    },
};

fn mk_array<T, const N: usize>(mut t: impl Iterator<Item = T>) -> impl Iterator<Item = [T; N]> {
    std::iter::from_fn(move || Some(core::array::from_fn(|_| t.next().unwrap())))
}

fn main() -> Result<(), RHDLError> {
    // First create an iterator of random nibbles
    let source_rng = XorShift128::default().map(|x| bits((x & 0xF) as u128));
    // Clone it so we can reference at the output of the pipe
    let mut dest_rng = source_rng.clone();
    // Transform the iterator to produce arrays.
    let source_rng = mk_array(source_rng);
    // Wrap the source into a stalling iterator to minic starvation
    let stalling_source = stalling(source_rng, 0.2);
    // Create the unit to test
    let uut = Flatten::<b4, 2, 4>::default();
    // Create the consumption function.  We should get the elements
    // back in the order they were generated
    let mut det = DetRng::new(0x1000);
    let consume = move |v: SinkView<_>| {
        if let Some(data) = v.accepted {
            let validation = dest_rng.next().unwrap();
            assert_eq!(data, validation);
        }
        // Issue a stall with some low probability.
        det.chance(70)
    };
    let uut = single_stage(uut, stalling_source, consume);
    // Feed the UUT a steady diet of clock pulses signals
    let input = std::iter::repeat_n((), 15)
        .with_reset(1)
        .clock_pos_edge(100);
    let vcd = uut.run(input).collect::<SvgFile>();
    rhdl_fpga::doc::write_svg_as_markdown(
        vcd,
        "flatten.md",
        SvgOptions::default().with_filter("(^top.uut.input.data)|(^top.uut.outputs.data)"),
    )?;
    Ok(())
}
