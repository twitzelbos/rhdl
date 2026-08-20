#[cfg(test)]
mod tests {
    use rhdl::prelude::*;
    use rhdl_fpga::cdc::cross_counter::{CrossCounter, In};
    use rhdl_fpga::doc::DetRng;

    /// Writes `time_tracing_waveform.svg`, which the book includes.
    ///
    /// The stimulus is a **seeded** generator, not `rand::random`. With
    /// random pulses this test rewrote the committed SVG with different
    /// bytes on every `cargo test`, so the working tree was never clean
    /// and — the part that costs something — a genuine change to the
    /// tracer was indistinguishable from the churn. That is the failure
    /// mode the 2026-08-16 "deterministic stimulus everywhere" work
    /// removed across `rhdl-fpga`; this crate was missed by it.
    ///
    /// Made deterministic in place rather than moved to an example,
    /// following that work's own precedent: the write *is* the point here,
    /// and with a fixed seed it is idempotent.
    #[test]
    fn make_trace_waveform() -> Result<(), RHDLError> {
        // ANCHOR: time_tracing_waveform
        // Start with a stream of pulses.  A seeded generator, so the
        // committed waveform regenerates byte-identically.
        let mut rng = DetRng::new(0x5EED_1234);
        let red = (0..100).map(move |_| rng.chance(50)).take(100);
        // Clock them on the red domain
        let red = red.with_reset(1).clock_pos_edge(100);
        // Create an empty stream on the blue domain
        let blue = std::iter::repeat(()).with_reset(1).clock_pos_edge(79);
        // Merge them
        let inputs = merge_map(red, blue, |r: (ClockReset, bool), b: (ClockReset, ())| In {
            incr: signal(r.1),
            incr_cr: signal(r.0),
            cr: signal(b.0),
        });
        // Next we create an instance of the clock-domain crossing core, with
        // the appropriate clock domains.
        let uut = CrossCounter::<Red, Blue, 4>::default();
        // Simulate the crosser, and collect into a VCD
        let svg = uut
            .run(inputs)
            .take_while(|x| x.time <= 1000)
            .collect::<SvgFile>();
        let options = SvgOptions::default().with_io_filter();
        std::fs::write("time_tracing_waveform.svg", svg.to_string(&options)?)?;
        // ANCHOR_END: time_tracing_waveform
        Ok(())
    }
}
