//! Debouncer
//!
//! Filters mechanical bounce (or any short transient) on a noisy
//! single-bit input.  The output only changes when the input has been
//! observed *stable* — no transitions — for `N` consecutive clocks.
//! This is the canonical Tier-2 composition demo: it builds on
//! [super::edge_detector::EdgeDetector] (any-edge detection),
//! [super::pulse_stretcher::PulseStretcher] (settle timer), and a
//! single [super::dff::DFF] (the latched stable output).
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +---+Debouncer+---+
     |                 |
bool |                 | bool
+--->| input    output +--->
     |                 |
     +-----------------+
")]
//!
//!# Internals
//!
//! The internal block diagram captures the composition:
//!
#![doc = badascii_doc::badascii!(r"
                +-+EdgeDetector+
input +-------->|        any   +---+
       |        +--------------+   |
       |                           v
       |                +-+PulseStretcher+
       |                |  trigger      o+--+
       |        +------>|                |  |
       |                +----------------+  | (settle)
       |                                    |
       |                                +-+!+
       |                                |   |
       |                                v stable
       |                             +-+MUX+----+
       +---->                        |i (stable)|     +-+DFF+--+
                                     |          +---->|d      q+----> output
                  q.output (loop) -->|q (else)  |     |        |
                                     +----------+     +--------+
                                                          ^
                                                          |
                                                          +-- q.output (feedback)
")]
//!
//!# Behavior
//!
//! - On any transition of the input, the internal pulse stretcher is
//!   re-armed for `N` cycles.  While the stretcher is armed, the
//!   output is held at its previous value.
//! - When the stretcher times out (no transitions for `N` cycles),
//!   the input is sampled into the output flip flop.
//! - `N` is configured at construction time; the bit width parameter
//!   bounds the maximum settle length (`2^N - 1`).
//! - End-to-end latency from a clean rising edge on the input to the
//!   output going high is roughly `N + 3` cycles (the extra cycles
//!   come from the EdgeDetector and DFF pipeline registers).
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/debouncer.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/debouncer.md")]
use rhdl::prelude::*;

use super::{dff, edge_detector::EdgeDetector, pulse_stretcher::PulseStretcher};

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// Debouncer core.
///
/// `N` is the bit width of the internal settle counter; the maximum
/// settle length is `2^N - 1` cycles.  The actual settle threshold is
/// supplied at construction time via [Self::new].
pub struct Debouncer<const N: usize>
where
    rhdl::bits::W<N>: BitWidth,
{
    edge: EdgeDetector,
    settle: PulseStretcher<N>,
    output: dff::DFF<bool>,
}

impl<const N: usize> Debouncer<N>
where
    rhdl::bits::W<N>: BitWidth,
{
    /// Create a new debouncer that requires `settle_cycles` of
    /// stability before propagating an input change to the output.
    ///
    /// A `settle_cycles` value of zero produces a degenerate
    /// debouncer that latches every input change immediately.
    pub fn new(settle_cycles: Bits<N>) -> Self {
        Self {
            edge: EdgeDetector::default(),
            settle: PulseStretcher::new(settle_cycles),
            output: dff::DFF::default(),
        }
    }
}

impl<const N: usize> SynchronousIO for Debouncer<N>
where
    rhdl::bits::W<N>: BitWidth,
{
    type I = bool;
    type O = bool;
    type Kernel = debouncer<N>;
}

#[kernel]
/// Kernel for [Debouncer].
pub fn debouncer<const N: usize>(cr: ClockReset, i: bool, q: Q<N>) -> (bool, D<N>)
where
    rhdl::bits::W<N>: BitWidth,
{
    let mut d = D::<N>::dont_care();
    // Edge detector watches the raw input.
    d.edge = i;
    // Any edge re-arms the settle timer.
    d.settle = q.edge.any;
    // Input is considered stable when there is no edge this cycle AND
    // the settle timer has fully expired.  The `q.edge.any` term is
    // load-bearing — without it, the very first transition leaks
    // through before the pulse-stretcher counter has had a chance to
    // arm (the stretcher's counter only updates on the next cycle).
    let stable = !q.settle && !q.edge.any;
    // Latch the current input only when stable; otherwise hold.
    d.output = if stable { i } else { q.output };
    let o = q.output;
    if cr.reset.any() {
        d.output = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    /// Build the Q value for direct kernel testing.  Inputs to the
    /// kernel are the *current* sub-circuit outputs, so we can pose
    /// "what does the kernel decide?" questions without running a
    /// full simulation.
    fn make_q(prev_output: bool, settle_active: bool, any_edge: bool) -> Q<4> {
        Q::<4> {
            edge: super::super::edge_detector::Edges {
                rising: false,
                falling: false,
                any: any_edge,
            },
            settle: settle_active,
            output: prev_output,
        }
    }

    // Tier 1 — direct kernel unit tests

    #[test]
    fn test_input_latched_when_stable() {
        let cr = ClockReset::dont_care();
        // Settle timer expired (stable), input low, prev output low.
        let q = make_q(false, false, false);
        let (_o, d) = debouncer::<4>(cr, false, q);
        // Input is sampled.
        assert!(!d.output);

        // Same setup but input high.
        let q = make_q(false, false, false);
        let (_o, d) = debouncer::<4>(cr, true, q);
        assert!(d.output);
    }

    #[test]
    fn test_input_held_when_not_stable() {
        let cr = ClockReset::dont_care();
        // Settle timer is still active; output should hold its prev value
        // regardless of input.
        let q = make_q(true, true, false);
        let (_o, d) = debouncer::<4>(cr, false, q);
        assert!(d.output, "output must hold prev when settle active");
    }

    #[test]
    fn test_any_edge_triggers_settle_arm() {
        let cr = ClockReset::dont_care();
        // Edge detector reports an edge — settle should be armed.
        let q = make_q(false, false, true);
        let (_o, d) = debouncer::<4>(cr, false, q);
        assert!(d.settle, "an edge must arm the settle timer");
    }

    #[test]
    fn test_no_edge_does_not_arm_settle() {
        let cr = ClockReset::dont_care();
        let q = make_q(false, false, false);
        let (_o, d) = debouncer::<4>(cr, false, q);
        assert!(!d.settle, "no edge -> settle stays disarmed");
    }

    #[test]
    fn test_reset_clears_latched_output() {
        let cr = clock_reset(clock(true), reset(true));
        let q = make_q(true, false, false);
        // Even with input high and stable, reset should override.
        let (_o, d) = debouncer::<4>(cr, true, q);
        assert!(!d.output);
    }

    // Tier 2 — iterator simulation

    /// A single high pulse shorter than the settle threshold should be
    /// rejected: the output never goes high.
    #[test]
    fn test_short_glitch_rejected() -> miette::Result<()> {
        const SETTLE: u128 = 5;
        // Input: glitch of 2 high cycles, then back to low forever.
        let mut input = vec![false; 40];
        input[3] = true;
        input[4] = true;
        let stream = input.iter().copied().with_reset(1).clock_pos_edge(100);
        let uut = Debouncer::<4>::new(bits(SETTLE));
        let outputs = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect::<Vec<_>>();
        assert!(
            outputs.iter().all(|x| !*x),
            "short glitch must not propagate, got {outputs:?}"
        );
        Ok(())
    }

    /// A sustained high input (well past the settle window) should
    /// eventually cause the output to go high and stay high.
    #[test]
    fn test_sustained_high_propagates() -> miette::Result<()> {
        const SETTLE: u128 = 5;
        // High starting at cycle 3 and held through the rest of the run.
        let mut input = vec![false; 40];
        for x in input.iter_mut().skip(3) {
            *x = true;
        }
        let stream = input.iter().copied().with_reset(1).clock_pos_edge(100);
        let uut = Debouncer::<4>::new(bits(SETTLE));
        let outputs = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect::<Vec<_>>();
        // Output must go high at some point and stay high once it does.
        let first_high = outputs.iter().position(|x| *x);
        assert!(first_high.is_some(), "output never went high: {outputs:?}");
        let first_high = first_high.unwrap();
        assert!(
            outputs.iter().skip(first_high).all(|x| *x),
            "output dropped low after going high: {outputs:?}"
        );
        Ok(())
    }

    // Tier 3 — HDL emission snapshot
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Debouncer::<4>::new(bits(5));
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        // The Verilog is large (composes three sub-cores) so we just
        // check it elaborates by asking for the pretty form.  Length
        // alone catches accidental codegen-output regressions.
        let expect = expect!["6988"];
        expect.assert_eq(&hdl.len().to_string());
        Ok(())
    }

    // Tier 4 — iverilog round-trip (RTL and NTL)
    #[test]
    fn test_debouncer_hdl_works() -> miette::Result<()> {
        let uut = Debouncer::<4>::new(bits(5));
        let mut input = vec![false; 40];
        input[3] = true;
        input[4] = true;
        for x in input.iter_mut().skip(15) {
            *x = true;
        }
        let stream = input.iter().copied().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    // Tier 5 — VCD digest
    #[test]
    fn test_debouncer_trace() -> miette::Result<()> {
        let uut = Debouncer::<4>::new(bits(5));
        let mut input = vec![false; 40];
        input[3] = true;
        input[4] = true;
        for x in input.iter_mut().skip(15) {
            *x = true;
        }
        let stream = input.iter().copied().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("debouncer");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["0082cb20217821e8268d2fb59b7aa3020c225b809084ca5e886cc1ae6f95d2de"];
        let digest = vcd.dump_to_file(root.join("debouncer.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
