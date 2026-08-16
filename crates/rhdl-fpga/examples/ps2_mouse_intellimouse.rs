use rhdl::prelude::*;
use rhdl_fpga::{
    doc::{write_fsm_diagram, write_svg_as_markdown},
    serial_bus::ps2_mouse_intellimouse::{In, Ps2MouseIntelliMouse},
};

fn main() -> Result<(), RHDLError> {
    write_fsm_diagram::<Ps2MouseIntelliMouse>("ps2_mouse_intellimouse_fsm.md")?;
    let mut stream_in: Vec<In> = Vec::new();
    for &b in &[0x08u8, 5, 10, 0x01, 0x09, 0xFE, 0xFF, 0x0F] {
        stream_in.push(In {
            byte_in: bits::<8>(b as u128),
            byte_valid: true,
        });
        stream_in.push(In {
            byte_in: bits(0),
            byte_valid: false,
        });
    }
    for _ in 0..6 {
        stream_in.push(In {
            byte_in: bits(0),
            byte_valid: false,
        });
    }
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let uut = Ps2MouseIntelliMouse::default();
    let svg = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(svg, "ps2_mouse_intellimouse.md", SvgOptions::default())?;
    Ok(())
}
