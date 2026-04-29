use rhdl::prelude::*;
use rhdl_fpga::{core::debouncer::Debouncer, doc::write_svg_as_markdown};

fn main() -> Result<(), RHDLError> {
    // A noisy input: a short glitch (rejected), then a sustained high
    // (propagates), then bouncing transitions (rejected during bounce,
    // settled value at the end).
    let mut pattern = vec![false; 40];
    // Brief glitch.
    pattern[3] = true;
    pattern[4] = true;
    // Sustained high from cycle 12 onward (propagates after settle).
    for x in pattern.iter_mut().take(30).skip(12) {
        *x = true;
    }
    // Bounce off again starting at cycle 30.
    pattern[30] = false;
    pattern[31] = true;
    pattern[32] = false;
    pattern[33] = true;
    pattern[34] = false;
    let input = pattern.into_iter().with_reset(1).clock_pos_edge(100);
    let uut = Debouncer::<4>::new(bits(5));
    let vcd = uut.run(input).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.*)|(^top.output.*)|(^top.clock.*)|(^top.reset.*)")
        .with_label_width(20);
    write_svg_as_markdown(vcd, "debouncer.md", options)?;
    Ok(())
}
