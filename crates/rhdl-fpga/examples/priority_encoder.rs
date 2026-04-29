use rhdl::prelude::*;
use rhdl_fpga::{core::priority_encoder::priority_encoder_lsb, doc::write_svg_as_markdown};

// The priority encoder is a pure combinational kernel.  The [Func]
// wrapper turns it into a synchronous-style core for trace generation.
#[kernel]
pub fn wrap_pe(_cr: ClockReset, input: Bits<8>) -> Option<Bits<3>> {
    priority_encoder_lsb::<8, 3>(input)
}

fn main() -> Result<(), RHDLError> {
    let uut: Func<Bits<8>, Option<Bits<3>>> = Func::try_new::<wrap_pe>()?;
    let inputs = [
        bits(0b0000_0000),
        bits(0b0000_0001),
        bits(0b0000_0010),
        bits(0b0000_1000),
        bits(0b1000_0000),
        bits(0b1010_1010),
        bits(0b0101_0101),
        bits(0b1111_0000),
    ]
    .into_iter()
    .with_reset(1)
    .clock_pos_edge(100);
    let vcd = uut.run(inputs).collect::<SvgFile>();
    let options = SvgOptions::default().with_label_width(20);
    write_svg_as_markdown(vcd, "priority_encoder.md", options)?;
    Ok(())
}
