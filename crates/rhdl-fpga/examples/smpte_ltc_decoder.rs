use rhdl::prelude::*;
use rhdl_fpga::{
    doc::{write_fsm_diagram, write_svg_as_markdown},
    serial_bus::smpte_ltc_decoder::{In, SmpteLtcDecoder},
};

fn bm_encode(bits_in: &[bool], cell_cycles: usize, start_level: bool) -> Vec<bool> {
    let mut out = Vec::new();
    let mut level = start_level;
    for &b in bits_in {
        level = !level;
        for _ in 0..(cell_cycles / 2) {
            out.push(level);
        }
        if b {
            level = !level;
        }
        for _ in 0..(cell_cycles / 2) {
            out.push(level);
        }
    }
    out
}

fn ltc_frame_bits(hh: u32, mm: u32, ss: u32, ff: u32) -> Vec<bool> {
    let mut bits = Vec::with_capacity(80);
    let push_bits = |bits: &mut Vec<bool>, value: u128, width: usize| {
        for k in 0..width {
            bits.push(((value >> k) & 1) != 0);
        }
    };
    push_bits(&mut bits, (ff % 10) as u128, 4);
    push_bits(&mut bits, 0, 4);
    push_bits(&mut bits, (ff / 10) as u128, 2);
    bits.push(false); // drop-frame
    bits.push(false); // colour-frame
    push_bits(&mut bits, 0, 4);
    push_bits(&mut bits, (ss % 10) as u128, 4);
    push_bits(&mut bits, 0, 4);
    push_bits(&mut bits, (ss / 10) as u128, 3);
    bits.push(false);
    push_bits(&mut bits, 0, 4);
    push_bits(&mut bits, (mm % 10) as u128, 4);
    push_bits(&mut bits, 0, 4);
    push_bits(&mut bits, (mm / 10) as u128, 3);
    bits.push(false);
    push_bits(&mut bits, 0, 4);
    push_bits(&mut bits, (hh % 10) as u128, 4);
    push_bits(&mut bits, 0, 4);
    push_bits(&mut bits, (hh / 10) as u128, 2);
    bits.push(false);
    bits.push(false);
    push_bits(&mut bits, 0, 4);
    push_bits(&mut bits, 0xBFFC, 16);
    bits
}

fn main() -> Result<(), RHDLError> {
    write_fsm_diagram::<SmpteLtcDecoder<14>>("smpte_ltc_decoder_fsm.md")?;

    let cell_cycles = 16;
    // Send 2 frames so the decoder warms up + locks before parsing.
    let mut all_bits = Vec::new();
    for _ in 0..2 {
        all_bits.extend(ltc_frame_bits(11, 22, 33, 5));
    }
    let line = bm_encode(&all_bits, cell_cycles, false);
    let stream_in: Vec<In> = line.iter().map(|&b| In { line_in: b }).collect();
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);

    let uut = SmpteLtcDecoder::<14>::default();
    let svg = uut.run(stream).collect::<SvgFile>();
    let opts = SvgOptions::default();
    write_svg_as_markdown(svg, "smpte_ltc_decoder.md", opts)?;
    Ok(())
}
