use rhdl::prelude::*;
use rhdl_fpga::doc::DetRng;
use rhdl_fpga::{core::counter::Counter, doc::write_svg_as_markdown};

fn main() -> Result<(), RHDLError> {
    let mut det = DetRng::new(0x1000);
    let input = (0..)
        .map(|_| det.chance(50))
        .with_reset(1)
        .clock_pos_edge(100);
    let uut = Counter::<4>::default();
    let vcd = uut
        .run(input)
        .take_while(|t| t.time < 1000)
        .collect::<SvgFile>();
    let options: SvgOptions = SvgOptions::default();
    write_svg_as_markdown(vcd, "counter.md", options)?;
    Ok(())
}
