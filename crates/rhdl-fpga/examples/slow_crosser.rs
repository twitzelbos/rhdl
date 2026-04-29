use rhdl::prelude::*;
use rhdl_fpga::{
    cdc::slow_crosser::{In, SlowCrosser},
    doc::write_svg_as_markdown,
};

fn main() -> Result<(), RHDLError> {
    // Send three values across the bridge with plenty of idle time
    // between sends so each crossing has time to complete.
    let mut src_pattern: Vec<(Bits<8>, bool)> = Vec::new();
    let values = [bits::<8>(0xA5), bits(0x5A), bits(0xFF)];
    for &v in &values {
        src_pattern.push((v, true));
        for _ in 0..30 {
            src_pattern.push((v, false));
        }
    }
    let red = src_pattern.into_iter().with_reset(2).clock_pos_edge(100);
    let blue = std::iter::repeat(false).with_reset(2).clock_pos_edge(79);
    let input = red.merge_map(blue, |r, b| In {
        src_data: signal(r.1 .0),
        src_send: signal(r.1 .1),
        src_cr: signal(r.0),
        dst_cr: signal(b.0),
    });
    let uut = SlowCrosser::<Bits<8>, Red, Blue>::default();
    let vcd = uut.run(input).take(200).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.*)|(^top.output.*)|(^top.src_clock.*)|(^top.dst_clock.*)|(^top.data_out.*)|(^top.busy.*)")
        .with_label_width(20);
    write_svg_as_markdown(vcd, "slow_crosser.md", options)?;
    Ok(())
}
