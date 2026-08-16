//! A small but genuinely rule-shaped widget: a toggle flip-flop
//! that responds to one of three commands (clear / set / toggle).
//!
//! Naturally expressed as 3 rules — one per command — all writing
//! the same register.  Phase-1.5 priority chain handles the
//! pairwise conflicts (write-write on `state`).  The guards are
//! mutually exclusive at runtime (each cycle the input is exactly
//! one variant), but the macro treats the rules as conflicting and
//! the priority chain handles the suppression cleanly.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel;

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub enum ToggleEvent {
    #[default]
    Hold,
    Clear,
    Set,
    Toggle,
}

rule_kernel! {
    pub struct ToggleFF {
        state: dff::DFF<bool>,
    }

    impl ToggleFF {
        #[rule(priority = 0)]
        fn clear(ctx: &mut RuleCtx<Self>, ev: ToggleEvent) {
            guard!(ev == ToggleEvent::Clear);
            set!(ctx.state, false);
        }

        #[rule(priority = 1)]
        fn set_high(ctx: &mut RuleCtx<Self>, ev: ToggleEvent) {
            guard!(ev == ToggleEvent::Set);
            set!(ctx.state, true);
        }

        #[rule(priority = 2)]
        fn toggle(ctx: &mut RuleCtx<Self>, ev: ToggleEvent) {
            guard!(ev == ToggleEvent::Toggle);
            set!(ctx.state, !*ctx.state);
        }

        #[output]
        fn output(self_q: &Self, _ev: ToggleEvent) -> bool {
            *self_q.state
        }
    }
}

#[test]
fn toggle_ff_responds_to_set() {
    let uut: ToggleFF = ToggleFF::default();
    let stream_in: Vec<ToggleEvent> = vec![ToggleEvent::Hold, ToggleEvent::Set, ToggleEvent::Hold];
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let final_state = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output)
        .unwrap_or(false);
    assert!(final_state, "expected state=true after Set command");
}

#[test]
fn toggle_ff_responds_to_clear() {
    let uut: ToggleFF = ToggleFF::default();
    // Set, then Clear, then Hold.
    let stream_in: Vec<ToggleEvent> = vec![
        ToggleEvent::Set,
        ToggleEvent::Hold,
        ToggleEvent::Clear,
        ToggleEvent::Hold,
    ];
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let final_state = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output)
        .unwrap_or(true);
    assert!(!final_state, "expected state=false after Clear");
}

#[test]
fn toggle_ff_toggles() {
    let uut: ToggleFF = ToggleFF::default();
    // Hold, Toggle, Hold, Toggle, Hold — should end at false → true → false → true (depending on timing).
    let stream_in: Vec<ToggleEvent> = vec![
        ToggleEvent::Hold,
        ToggleEvent::Toggle,
        ToggleEvent::Hold,
        ToggleEvent::Toggle,
        ToggleEvent::Hold,
    ];
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let outputs: Vec<_> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    // The state was toggled twice, so it should end up where it started (false).
    assert_eq!(
        *outputs.last().unwrap(),
        false,
        "expected state=false after two toggles; got outputs={outputs:?}",
    );
}
