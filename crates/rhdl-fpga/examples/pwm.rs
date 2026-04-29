use rhdl::prelude::*;
use rhdl_fpga::{core::pwm::PwmGenerator, doc::write_svg_as_markdown};

fn main() -> Result<(), RHDLError> {
    let mut pattern: Vec<Bits<4>> = Vec::new();
    for _ in 0..16 {
        pattern.push(bits(4));
    }
    for _ in 0..16 {
        pattern.push(bits(8));
    }
    for _ in 0..16 {
        pattern.push(bits(12));
    }
    let stream = pattern.into_iter().with_reset(1).clock_pos_edge(100);
    let uut = PwmGenerator::<4>::default();
    let vcd = uut.run(stream).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter(
            "(^top.input.*)|(^top.output.*)|(^top.clock.*)|(^top.reset.*)|(^top.counter.*)",
        )
        .with_label_width(20);
    write_svg_as_markdown(vcd, "pwm.md", options)?;
    Ok(())
}
