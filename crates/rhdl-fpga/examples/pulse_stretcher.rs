use rhdl::prelude::*;
use rhdl_fpga::{core::pulse_stretcher::PulseStretcher, doc::write_svg_as_markdown};

fn main() -> Result<(), RHDLError> {
    // Two short input pulses and one re-trigger to demonstrate both
    // basic stretching and the level-retriggerable behavior.
    let mut pattern = vec![false; 24];
    pattern[2] = true;
    pattern[10] = true;
    pattern[12] = true;
    let input = pattern.into_iter().with_reset(1).clock_pos_edge(100);
    let uut = PulseStretcher::<4>::new(bits(5));
    let vcd = uut.run(input).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter(
            "(^top.input.*)|(^top.output.*)|(^top.clock.*)|(^top.reset.*)|(^top.counter.*)",
        )
        .with_label_width(20);
    write_svg_as_markdown(vcd, "pulse_stretcher.md", options)?;
    Ok(())
}
