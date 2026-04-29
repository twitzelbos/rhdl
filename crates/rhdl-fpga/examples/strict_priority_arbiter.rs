use rhdl::prelude::*;
use rhdl_fpga::{core::strict_priority_arbiter::StrictPriorityArbiter, doc::write_svg_as_markdown};

fn main() -> Result<(), RHDLError> {
    let inputs: Vec<Bits<4>> = vec![
        bits(0b0001), // bit 0 only
        bits(0b1110), // bits 1, 2, 3 — bit 1 wins
        bits(0b1100), // bits 2, 3 — bit 2 wins
        bits(0b1000), // bit 3 only
        bits(0b1111), // bit 0 wins (lowest)
        bits(0b0000), // no requests
        bits(0b0101), // bits 0, 2 — bit 0 wins
    ];
    let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
    let uut = StrictPriorityArbiter::<4, 2>::default();
    let vcd = uut.run(stream).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.*)|(^top.output.*)|(^top.clock.*)|(^top.reset.*)")
        .with_label_width(20);
    write_svg_as_markdown(vcd, "strict_priority_arbiter.md", options)?;
    Ok(())
}
