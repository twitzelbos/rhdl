// Byte-parallel CRC-32 over the algorithm's own check string.
//
// "123456789" is the canonical CRC-32 test vector; its checksum is
// 0xCBF43926, and that value is what pins the parameter set (reflected
// in and out, init 0xFFFFFFFF, final XOR 0xFFFFFFFF).
//
// What to look for:
//
//   - `clear` on the first cycle loads the all-ones initial remainder.
//     The reset value is deliberately not zero -- a zero-initialised
//     CRC-32 computes a different, wrong checksum rather than failing.
//   - `crc` lags `byte` by one cycle, because the remainder is
//     registered. The finished checksum appears on the cycle after the
//     last byte is accepted, which is the final sample here.
//   - One byte is consumed per cycle. That is the whole point of this
//     widget over `core::crc`: 8 bits per cycle at 125 MHz is exactly
//     1 Gbit/s, so it can keep up with a wire.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::{
    core::crc32::{Crc32, In},
    doc::write_svg_as_markdown,
};

fn main() -> Result<(), RHDLError> {
    let mut stream: Vec<In> = vec![In {
        byte: bits(0),
        enable: false,
        clear: true,
    }];
    for &byte in b"123456789" {
        stream.push(In {
            byte: bits(byte as u128),
            enable: true,
            clear: false,
        });
    }
    // One idle cycle so the last byte's checksum reaches the output.
    stream.push(In {
        byte: bits(0),
        enable: false,
        clear: false,
    });

    let uut = Crc32::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(input).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "crc32.md",
        SvgOptions::default().with_io_filter().with_label_width(20),
    )?;
    Ok(())
}
