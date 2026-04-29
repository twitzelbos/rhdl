use rhdl::prelude::*;
use rhdl_fpga::{core::round_robin_arbiter::RoundRobinArbiter, doc::write_svg_as_markdown};

fn main() -> Result<(), RHDLError> {
    // A varied request pattern to demonstrate fairness, gaps, and rotation.
    let inputs: Vec<Bits<4>> = vec![
        bits(0b1111),
        bits(0b1111),
        bits(0b1111),
        bits(0b1111),
        bits(0b0101),
        bits(0b0101),
        bits(0b0010),
        bits(0b0010),
        bits(0b0000),
        bits(0b1000),
        bits(0b1111),
        bits(0b0011),
    ];
    let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
    let uut = RoundRobinArbiter::<4, 2>::default();
    let vcd = uut.run(stream).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.*)|(^top.output.*)|(^top.clock.*)|(^top.reset.*)")
        .with_label_width(20);
    write_svg_as_markdown(vcd, "round_robin_arbiter.md", options)?;
    Ok(())
}
