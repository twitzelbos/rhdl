use rhdl::prelude::*;
use rhdl_fpga::{core::popcount::popcount, doc::write_svg_as_markdown};

#[kernel]
pub fn wrap(_cr: ClockReset, input: Bits<8>) -> Bits<4> {
    popcount::<8, 4>(input)
}

fn main() -> Result<(), RHDLError> {
    let uut: Func<Bits<8>, Bits<4>> = Func::try_new::<wrap>()?;
    let inputs = [
        bits(0b0000_0000),
        bits(0b0000_0001),
        bits(0b0000_0011),
        bits(0b0000_0111),
        bits(0b0000_1111),
        bits(0b0001_1111),
        bits(0b0011_1111),
        bits(0b0111_1111),
        bits(0b1111_1111),
        bits(0b1010_1010),
        bits(0b0101_0101),
    ]
    .into_iter()
    .with_reset(1)
    .clock_pos_edge(100);
    let vcd = uut.run(inputs).collect::<SvgFile>();
    let options = SvgOptions::default().with_label_width(20);
    write_svg_as_markdown(vcd, "popcount.md", options)?;
    Ok(())
}
