use rhdl::prelude::*;
use rhdl_fpga::{
    doc::{write_fsm_diagram, write_svg_as_markdown},
    serial_bus::ps2_keyboard_decoder::{In, Ps2KeyboardDecoder},
};

fn main() -> Result<(), RHDLError> {
    write_fsm_diagram::<Ps2KeyboardDecoder>("ps2_keyboard_decoder_fsm.md")?;

    // Sequence: 'A' make, 'A' break, Up arrow make, Up arrow break.
    let codes: &[u8] = &[0x1C, 0xF0, 0x1C, 0xE0, 0x75, 0xE0, 0xF0, 0x75];
    let mut stream_in: Vec<In> = Vec::new();
    for &c in codes {
        stream_in.push(In {
            scan_code: bits::<8>(c as u128),
            scan_valid: true,
        });
        stream_in.push(In {
            scan_code: bits(0),
            scan_valid: false,
        });
    }
    for _ in 0..6 {
        stream_in.push(In {
            scan_code: bits(0),
            scan_valid: false,
        });
    }
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let uut = Ps2KeyboardDecoder::default();
    let svg = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(svg, "ps2_keyboard_decoder.md", SvgOptions::default())?;
    Ok(())
}
