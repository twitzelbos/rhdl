use rhdl::prelude::*;
use rhdl_fpga::{
    core::register_file::{In, RegisterFile},
    doc::write_svg_as_markdown,
};

fn main() -> Result<(), RHDLError> {
    let mut stream_in: Vec<In<Bits<8>, 2>> = Vec::new();
    // Write each address sequentially.
    for addr in 0u128..4 {
        stream_in.push(In {
            read_addr: bits(0),
            read_enable: false,
            write_addr: bits(addr),
            write_data: bits(0xA0 + addr),
            write_enable: true,
        });
    }
    // Then read each address back.
    for addr in 0u128..4 {
        stream_in.push(In {
            read_addr: bits(addr),
            read_enable: true,
            write_addr: bits(0),
            write_data: bits(0),
            write_enable: false,
        });
    }
    let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
    let uut = RegisterFile::<Bits<8>, 4, 2>::default();
    let vcd = uut.run(stream).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.*)|(^top.output.*)|(^top.clock.*)|(^top.reset.*)")
        .with_label_width(20);
    write_svg_as_markdown(vcd, "register_file.md", options)?;
    Ok(())
}
