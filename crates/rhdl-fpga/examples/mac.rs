use rhdl::prelude::*;
use rhdl_fpga::{
    core::mac::{In, MacUnit},
    doc::write_svg_as_markdown,
};

fn main() -> Result<(), RHDLError> {
    let mut stream_in: Vec<In<8>> = vec![In {
        a: bits(0),
        b: bits(0),
        enable: false,
        clear: true,
    }];
    // Sum 1*3 + 2*6 + 3*9 + 4*12 + 5*15 = 3 + 12 + 27 + 48 + 75 = 165.
    for k in 1u128..6 {
        stream_in.push(In {
            a: bits(k),
            b: bits(k * 3),
            enable: true,
            clear: false,
        });
    }
    stream_in.push(In {
        a: bits(0),
        b: bits(0),
        enable: false,
        clear: false,
    });
    let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
    let uut = MacUnit::<8, 24>::default();
    let vcd = uut.run(stream).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.*)|(^top.output.*)|(^top.clock.*)|(^top.reset.*)|(^top.acc.*)")
        .with_label_width(20);
    write_svg_as_markdown(vcd, "mac.md", options)?;
    Ok(())
}
