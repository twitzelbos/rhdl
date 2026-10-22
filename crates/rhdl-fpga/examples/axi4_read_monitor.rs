use rhdl::prelude::*;
use rhdl_fpga::axi4::check::read_monitor::Axi4ReadMonitor;
use rhdl_fpga::axi4::check::traffic::{ReadViolation, read_transcript};
use rhdl_fpga::doc::write_svg_as_markdown;

// A 32-bit data bus.
type Monitor = Axi4ReadMonitor<32, 32, 4, 4>;

fn main() -> Result<(), RHDLError> {
    // A conformant four-beat read burst, then the same burst with a
    // beat bearing the wrong RID. The monitor latches the fault and
    // holds it, so `fault` changes once and stays changed.
    let mut cycles = read_transcript::<32, 32, 4>(4, ReadViolation::None);
    cycles.extend(read_transcript::<32, 32, 4>(4, ReadViolation::RIdMismatch));
    let input = cycles.into_iter().with_reset(1).clock_pos_edge(100);
    let uut = Monitor::default();
    let vcd = uut.run(input).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.ar_valid)|(^top.input.r_valid)|(^top.input.r.id)|(^top.output.*)")
        .with_label_width(24);
    write_svg_as_markdown(vcd, "axi4_read_monitor.md", options)?;
    Ok(())
}
