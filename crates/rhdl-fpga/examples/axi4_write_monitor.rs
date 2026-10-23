use rhdl::prelude::*;
use rhdl_fpga::axi4::check::monitor::Axi4WriteMonitor;
use rhdl_fpga::axi4::check::traffic::{Violation, write_transcript};
use rhdl_fpga::doc::write_svg_as_markdown;

// A 32-bit data bus: four byte lanes, so STRB is 4.
type Monitor = Axi4WriteMonitor<32, 32, 4, 4, 16>;

fn main() -> Result<(), RHDLError> {
    // A conformant four-beat burst, then the same burst with WLAST
    // asserted a beat early. The monitor latches the fault and holds
    // it, so `fault` changes once and stays changed -- which is the
    // sticky-first behaviour the trace is here to show.
    let mut cycles = write_transcript::<32, 32, 4, 4>(4, Violation::None);
    cycles.extend(write_transcript::<32, 32, 4, 4>(4, Violation::WLastEarly));
    let input = cycles.into_iter().with_reset(1).clock_pos_edge(100);
    let uut = Monitor::default();
    let vcd = uut.run(input).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter(
            "(^top.input.aw_valid)|(^top.input.w_valid)|(^top.input.w.last)|(^top.output.*)",
        )
        .with_label_width(24);
    write_svg_as_markdown(vcd, "axi4_write_monitor.md", options)?;
    Ok(())
}
