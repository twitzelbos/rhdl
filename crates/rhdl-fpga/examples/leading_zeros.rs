use rhdl::prelude::*;
use rhdl_fpga::{core::leading_zeros::leading_zeros, doc::write_svg_as_markdown};

#[kernel]
pub fn wrap(_cr: ClockReset, input: Bits<8>) -> Bits<4> {
    leading_zeros::<8, 4>(input)
}

fn main() -> Result<(), RHDLError> {
    let uut: Func<Bits<8>, Bits<4>> = Func::try_new::<wrap>()?;
    let inputs = [
        bits(0b0000_0000),
        bits(0b0000_0001),
        bits(0b0000_0010),
        bits(0b0000_0100),
        bits(0b0001_0000),
        bits(0b0100_0000),
        bits(0b1000_0000),
        bits(0b1111_1111),
        bits(0b0010_1010),
    ]
    .into_iter()
    .with_reset(1)
    .clock_pos_edge(100);
    let vcd = uut.run(inputs).collect::<SvgFile>();
    let options = SvgOptions::default().with_label_width(20);
    write_svg_as_markdown(vcd, "leading_zeros.md", options)?;
    Ok(())
}
