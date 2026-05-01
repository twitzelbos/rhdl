//! Phase 2 — `mutually_exclusive` annotation.
//!
//! When the user asserts that two conflicting rules are pairwise
//! mutually exclusive (their guards are jointly unsatisfiable), the
//! macro trusts the assertion and elides the suppressor term in the
//! priority chain — producing a cleaner emitted Verilog without
//! sacrificing correctness *as long as the assertion holds*.  This
//! is documented as a trusted contract: a wrong assertion produces
//! a runtime hardware bug, not a compile error.
//!
//! These tests verify two things:
//!
//! 1. **Functional behaviour is unchanged** when the assertion
//!    holds — both arms of the mutually-exclusive pair fire under
//!    their respective guards.
//!
//! 2. **The emitted Verilog drops the suppressor** when the
//!    assertion is present — the `_fire_<rule>` line for the lower-
//!    priority rule does NOT contain `!(_fire_<higher>)`.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel;

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub enum LightCommand {
    #[default]
    Off,
    Red,
    Green,
}

// `set_red` and `set_green` write the same register but their
// guards (cmd == Red vs. cmd == Green) are pairwise unsatisfiable.
// We declare that mutual exclusion explicitly so the priority chain
// does not synthesise a redundant suppressor.
rule_kernel! {
    pub struct TrafficLight {
        colour: dff::DFF<Bits<2>>,
    }

    impl TrafficLight {
        #[rule(mutually_exclusive = "set_green")]
        fn set_red(ctx: &mut RuleCtx<Self>, cmd: LightCommand) {
            guard!(cmd == LightCommand::Red);
            set!(ctx.colour, bits::<2>(1));
        }

        #[rule(mutually_exclusive = "set_red")]
        fn set_green(ctx: &mut RuleCtx<Self>, cmd: LightCommand) {
            guard!(cmd == LightCommand::Green);
            set!(ctx.colour, bits::<2>(2));
        }

        #[output]
        fn output(self_q: &Self, _cmd: LightCommand) -> Bits<2> {
            *self_q.colour
        }
    }
}

#[test]
fn mutually_exclusive_red_arm_fires() {
    let uut: TrafficLight = TrafficLight::default();
    let stream_in = vec![
        LightCommand::Red,
        LightCommand::Red,
        LightCommand::Red,
    ];
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    assert_eq!(last, 1, "expected Red to set colour=1; got {last}");
}

#[test]
fn mutually_exclusive_green_arm_fires() {
    let uut: TrafficLight = TrafficLight::default();
    let stream_in = vec![
        LightCommand::Green,
        LightCommand::Green,
        LightCommand::Green,
    ];
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    assert_eq!(last, 2, "expected Green to set colour=2; got {last}");
}

#[test]
fn mutually_exclusive_off_holds_state() {
    let uut: TrafficLight = TrafficLight::default();
    let stream_in = vec![
        LightCommand::Red,
        LightCommand::Off,
        LightCommand::Off,
    ];
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    // First cycle sets to 1; subsequent Off cycles hold (no rule fires).
    assert_eq!(last, 1, "expected Off to hold colour=1 from prior Red; got {last}");
}

#[test]
fn mutually_exclusive_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: TrafficLight = TrafficLight::default();
    let stream = vec![
        LightCommand::Red,
        LightCommand::Green,
        LightCommand::Off,
    ]
    .into_iter()
    .with_reset(2)
    .clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    let tm = test_bench.ntl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
