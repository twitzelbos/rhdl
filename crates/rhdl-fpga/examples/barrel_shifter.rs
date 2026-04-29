use rhdl::prelude::*;
use rhdl_fpga::{
    core::barrel_shifter::{barrel_shifter, ShiftOp},
    doc::write_svg_as_markdown,
};

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct In {
    pub data: Bits<8>,
    pub amount: Bits<4>,
    pub op: ShiftOp,
}

#[kernel]
pub fn wrap(_cr: ClockReset, i: In) -> Bits<8> {
    barrel_shifter::<8, 4>(i.data, i.amount, i.op)
}

fn main() -> Result<(), RHDLError> {
    let uut: Func<In, Bits<8>> = Func::try_new::<wrap>()?;
    // Demonstrate each operation on the same input pattern.
    let inputs = [
        In {
            data: bits(0xA5),
            amount: bits(0),
            op: ShiftOp::LogicalLeft,
        },
        In {
            data: bits(0xA5),
            amount: bits(2),
            op: ShiftOp::LogicalLeft,
        },
        In {
            data: bits(0xA5),
            amount: bits(4),
            op: ShiftOp::LogicalLeft,
        },
        In {
            data: bits(0xA5),
            amount: bits(4),
            op: ShiftOp::LogicalRight,
        },
        In {
            data: bits(0xA5),
            amount: bits(4),
            op: ShiftOp::ArithmeticRight,
        },
        In {
            data: bits(0x55),
            amount: bits(4),
            op: ShiftOp::ArithmeticRight,
        },
        In {
            data: bits(0xA5),
            amount: bits(1),
            op: ShiftOp::RotateLeft,
        },
        In {
            data: bits(0xA5),
            amount: bits(1),
            op: ShiftOp::RotateRight,
        },
    ]
    .into_iter()
    .with_reset(1)
    .clock_pos_edge(100);
    let vcd = uut.run(inputs).collect::<SvgFile>();
    let options = SvgOptions::default().with_label_width(20);
    write_svg_as_markdown(vcd, "barrel_shifter.md", options)?;
    Ok(())
}
