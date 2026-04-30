use rhdl::prelude::*;
use rhdl_fpga::{
    doc::{write_fsm_diagram, write_svg_as_markdown},
    serial_bus::midi_parser::{In, MidiParser},
};

fn byte_in(b: u8) -> In {
    In {
        byte_in: Some(bits::<8>(b as u128)),
    }
}
fn idle_in() -> In {
    In { byte_in: None }
}

fn main() -> Result<(), RHDLError> {
    write_fsm_diagram::<MidiParser>("midi_parser_fsm.md")?;

    // Send a Note On (3-byte channel-voice), then Program Change (2-byte),
    // then a SysEx, then a Real-Time Timing Clock interrupting a NoteOn.
    let bytes: Vec<u8> = vec![
        0x90, 60, 100, // Note On ch 0 note 60 vel 100
        0xC3, 42, // Program Change ch 3 prog 42
        0xF0, 0x7E, 0x01, 0xF7, // SysEx (3-byte body)
        0x91, 64, 0xF8, 80, // Note On ch 1 note 64 [TimingClock interrupting] vel 80
    ];
    let mut stream_in: Vec<In> = Vec::new();
    for b in bytes {
        stream_in.push(byte_in(b));
        stream_in.push(idle_in());
    }
    for _ in 0..6 {
        stream_in.push(idle_in());
    }
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let uut = MidiParser::default();
    let svg = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(svg, "midi_parser.md", SvgOptions::default())?;
    Ok(())
}
