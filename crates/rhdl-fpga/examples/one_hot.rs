use rhdl::prelude::*;
use rhdl_fpga::{core::one_hot::binary_to_one_hot, doc::write_svg_as_markdown};

#[kernel]
pub fn wrap(_cr: ClockReset, idx: Bits<3>) -> Bits<8> {
    binary_to_one_hot::<3, 8>(idx)
}

fn main() -> Result<(), RHDLError> {
    let uut: Func<Bits<3>, Bits<8>> = Func::try_new::<wrap>()?;
    let inputs = (0u128..8)
        .map(bits)
        .collect::<Vec<_>>()
        .into_iter()
        .with_reset(1)
        .clock_pos_edge(100);
    let vcd = uut.run(inputs).collect::<SvgFile>();
    let options = SvgOptions::default().with_label_width(20);
    write_svg_as_markdown(vcd, "one_hot.md", options)?;
    Ok(())
}
