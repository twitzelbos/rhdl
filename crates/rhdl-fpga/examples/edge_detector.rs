use rhdl::prelude::*;
use rhdl_fpga::{core::edge_detector::EdgeDetector, doc::write_svg_as_markdown};

fn main() -> Result<(), RHDLError> {
    // Deterministic input pattern with a mix of rising, falling, and held
    // levels so the trace is reproducible across runs.
    let pattern = [
        false, false, true, true, false, false, true, false, false, true, true, true, false, false,
    ];
    let input = pattern.into_iter().with_reset(1).clock_pos_edge(100);
    let uut = EdgeDetector::default();
    let vcd = uut.run(input).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.*)|(^top.outputs.*)|(^top.clock.*)|(^top.reset.*)")
        .with_label_width(20);
    write_svg_as_markdown(vcd, "edge_detector.md", options)?;
    Ok(())
}
