use rhdl::prelude::*;
use rhdl_fpga::{
    core::divider::{Divider, In},
    doc::write_svg_as_markdown,
};

fn main() -> Result<(), RHDLError> {
    let mut stream_in: Vec<In<8>> = vec![In {
        dividend: bits(100),
        divisor: bits(7),
        start: true,
    }];
    for _ in 0..12 {
        stream_in.push(In {
            dividend: bits(0),
            divisor: bits(0),
            start: false,
        });
    }
    let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
    let uut = Divider::<8, 4>::default();
    let vcd = uut.run(stream).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter(
            "(^top.input.*)|(^top.output.*)|(^top.clock.*)|(^top.reset.*)|(^top.counter.*)",
        )
        .with_label_width(20);
    write_svg_as_markdown(vcd, "divider.md", options)?;
    Ok(())
}
