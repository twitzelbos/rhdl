use rhdl::prelude::*;
use rhdl_fpga::{
    core::crc::{CrcEngine, In},
    doc::write_svg_as_markdown,
};

fn main() -> Result<(), RHDLError> {
    // Stream "abc" (24 bits, MSB-first) through CRC-16-CCITT.
    let bytes: &[u8] = b"abc";
    let mut bit_stream: Vec<In> = vec![In {
        bit: false,
        enable: false,
        clear: true,
    }];
    for &byte in bytes {
        for i in (0..8).rev() {
            bit_stream.push(In {
                bit: ((byte >> i) & 1) != 0,
                enable: true,
                clear: false,
            });
        }
    }
    bit_stream.push(In {
        bit: false,
        enable: false,
        clear: false,
    });
    let stream = bit_stream.into_iter().with_reset(1).clock_pos_edge(100);
    let uut = CrcEngine::<16>::new(bits(0x1021), bits(0xFFFF));
    let vcd = uut.run(stream).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.*)|(^top.output.*)|(^top.clock.*)|(^top.reset.*)")
        .with_label_width(20);
    write_svg_as_markdown(vcd, "crc.md", options)?;
    Ok(())
}
