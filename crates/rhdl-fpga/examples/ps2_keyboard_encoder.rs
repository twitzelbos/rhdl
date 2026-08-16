use rhdl::prelude::*;
use rhdl_fpga::{
    doc::{write_fsm_diagram, write_svg_as_markdown},
    serial_bus::ps2_keyboard_encoder::{In, Ps2KeyboardEncoder},
};
fn main() -> Result<(), RHDLError> {
    write_fsm_diagram::<Ps2KeyboardEncoder<8>>("ps2_keyboard_encoder_fsm.md")?;
    let mut stream_in: Vec<In> = vec![
        In {
            scancode: bits(0),
            make: false,
            extended: false,
            send: false,
            clk_in: true
        };
        2
    ];
    stream_in.push(In {
        scancode: bits(0x1C),
        make: true,
        extended: false,
        send: true,
        clk_in: true,
    });
    for _ in 0..400 {
        stream_in.push(In {
            scancode: bits(0),
            make: false,
            extended: false,
            send: false,
            clk_in: true,
        });
    }
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let uut = Ps2KeyboardEncoder::<8>::new(bits(4));
    let svg = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(svg, "ps2_keyboard_encoder.md", SvgOptions::default())?;
    Ok(())
}
