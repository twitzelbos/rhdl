use rhdl::prelude::*;
use rhdl_fpga::{
    cdc::synchronizer_chain::{BitSyncChain, In},
    doc::write_svg_as_markdown,
};

fn main() -> Result<(), RHDLError> {
    let red = [false, false, true, true, false, false, true, false]
        .into_iter()
        .with_reset(1)
        .clock_pos_edge(100);
    let blue = std::iter::repeat(false).with_reset(1).clock_pos_edge(79);
    let input = red.merge_map(blue, |r, b| In {
        data: signal(r.1),
        cr: signal(b.0),
    });
    let uut = BitSyncChain::<Red, Blue, 4>::default();
    let vcd = uut.run(input).take(80).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.*)|(^top.output.*)|(^top.clock.*)|(^top.reset.*)")
        .with_label_width(20);
    write_svg_as_markdown(vcd, "synchronizer_chain.md", options)?;
    Ok(())
}
