use rhdl::prelude::*;
use rhdl_fpga::{
    core::comparator::{Flags, comparator},
    doc::write_svg_as_markdown,
};

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct In {
    pub a: Bits<8>,
    pub b: Bits<8>,
}

#[kernel]
pub fn wrap(_cr: ClockReset, i: In) -> Flags {
    comparator::<8>(i.a, i.b)
}

fn main() -> Result<(), RHDLError> {
    let uut: Func<In, Flags> = Func::try_new::<wrap>()?;
    let inputs = [
        (0u128, 0u128),
        (1, 2),
        (2, 1),
        (42, 42),
        (255, 0),
        (0, 255),
        (100, 100),
        (200, 100),
        (50, 200),
    ]
    .into_iter()
    .map(|(a, b)| In {
        a: bits(a),
        b: bits(b),
    })
    .collect::<Vec<_>>()
    .into_iter()
    .with_reset(1)
    .clock_pos_edge(100);
    let vcd = uut.run(inputs).collect::<SvgFile>();
    let options = SvgOptions::default().with_label_width(20);
    write_svg_as_markdown(vcd, "comparator.md", options)?;
    Ok(())
}
