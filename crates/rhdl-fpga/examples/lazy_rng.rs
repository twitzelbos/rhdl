use rhdl::prelude::*;
use rhdl_fpga::doc::DetRng;
use rhdl_fpga::stream::{ready, testing::lazy_random::*};

fn main() -> Result<(), RHDLError> {
    let mut det = DetRng::new(0x1000);
    let input = (0..)
        .map(|_| det.chance(80))
        .map(|r| In { ready: ready(r) })
        .with_reset(1)
        .clock_pos_edge(100)
        .take_while(|t| t.time < 1500);
    let uut = LazyRng::default();
    let vcd = uut.run(input).collect::<SvgFile>();
    rhdl_fpga::doc::write_svg_as_markdown(
        vcd,
        "lazy_rng.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
