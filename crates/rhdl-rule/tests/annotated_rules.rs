//! Phase-1.5 acceptance tests:
//!
//! - **Explicit `#[rule(priority = N)]` annotation** overrides
//!   source-code order.  Lower N = higher priority.
//! - **Rules with no input parameter** (just `ctx`) are now
//!   accepted.
//! - **`#[rule(conflict_free = "other")]`** is validated against
//!   the computed conflict matrix at compile time.
//!
//! Mirrors the canonical example in `rule-architecture.md` §4.1
//! (CounterAndFlag with three rules including the input-less
//! `reset_on_max`).

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel;

// Each `rule_kernel!` invocation is wrapped in its own module
// because the SynchronousDQ derive generates `Q` and `D` types in
// the parent module namespace; multiple widgets in one module
// would collide.

// ===========================================================
// Test 1: Explicit priority overrides source order.
//
// Two rules with always-true guards, both writing the same register.
// The source-order priority would put `set_low` first; explicit
// priority puts `set_high` first.  Verify priority annotation wins.
// ===========================================================

mod explicit_priority {
    use super::*;

    rule_kernel! {
        pub struct ExplicitPriority {
            val: dff::DFF<Bits<8>>,
        }

        impl ExplicitPriority {
            // Source-order priority would place this first.  But the
            // explicit annotation below makes it lower-priority.
            #[rule(priority = 1)]
            fn set_low(ctx: &mut RuleCtx<Self>, _flag: bool) {
                set!(ctx.val, bits::<8>(7));
            }

            // Explicit priority 0 = highest.  Should always fire and
            // suppress `set_low` (write-write conflict on `val`).
            #[rule(priority = 0)]
            fn set_high(ctx: &mut RuleCtx<Self>, _flag: bool) {
                set!(ctx.val, bits::<8>(99));
            }

            #[output]
            fn output(self_q: &Self, _flag: bool) -> Bits<8> {
                *self_q.val
            }
        }
    }
}

#[test]
fn explicit_priority_overrides_source_order() {
    use explicit_priority::ExplicitPriority;
    let uut: ExplicitPriority = ExplicitPriority::default();
    let stream = std::iter::repeat_n(true, 5)
        .with_reset(2)
        .clock_pos_edge(100);
    let final_value = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    assert_eq!(
        final_value, 99,
        "expected explicit-priority-0 rule (set_high) to win; got {final_value}.  \
         Source-order would give 7; this test proves the annotation is used.",
    );
}

// ===========================================================
// Test 2: Rule with no input parameter (just `ctx`).
//
// A counter that only counts when below 10.  The `cap` rule resets
// the counter to 0 once it hits the cap; it takes no input.
// ===========================================================

mod capped_counter {
    use super::*;

    rule_kernel! {
        pub struct CappedCounter {
            counter: dff::DFF<Bits<8>>,
        }

        impl CappedCounter {
            // Highest-priority rule; takes NO input parameter.
            // Resets the counter when it hits 5.
            #[rule(priority = 0)]
            fn reset_at_cap(ctx: &mut RuleCtx<Self>) {
                guard!(*ctx.counter == bits::<8>(5));
                set!(ctx.counter, bits::<8>(0));
            }

            // Lower-priority rule; increments when enable is high.
            // Conflicts with reset_at_cap (both write counter), so when
            // counter == 5 the priority chain suppresses this one.
            #[rule(priority = 1)]
            fn increment(ctx: &mut RuleCtx<Self>, enable: bool) {
                guard!(enable);
                set!(ctx.counter, *ctx.counter + bits::<8>(1));
            }

            #[output]
            fn output(self_q: &Self, _enable: bool) -> Bits<8> {
                *self_q.counter
            }
        }
    }
}

#[test]
fn no_input_rule_with_priority_chain() {
    use capped_counter::CappedCounter;
    let uut: CappedCounter = CappedCounter::default();
    // Run with enable=true for many cycles; the counter should
    // count 0, 1, 2, 3, 4, 5, then immediately reset to 0 (because
    // reset_at_cap suppresses increment when counter == 5), then
    // count back up.  We validate that the counter never exceeds 5.
    let stream = std::iter::repeat_n(true, 30)
        .with_reset(2)
        .clock_pos_edge(100);
    let max_seen = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output.raw())
        .max()
        .unwrap_or(0);
    assert_eq!(
        max_seen, 5,
        "expected counter to be capped at 5; reached {max_seen}",
    );
}

// ===========================================================
// Test 3: `conflict_free` annotation accepted when truly
// conflict-free.  Two rules that touch *different* registers — no
// conflict — and the assertion passes compile-time validation.
// ===========================================================

mod two_independent_counters {
    use super::*;

    rule_kernel! {
        pub struct TwoIndependentCounters {
            a: dff::DFF<Bits<8>>,
            b: dff::DFF<Bits<8>>,
        }

        impl TwoIndependentCounters {
            // Touches `a` only.  Asserts conflict-free with the rule
            // touching `b` only — true by construction.
            #[rule(conflict_free = "bump_b")]
            fn bump_a(ctx: &mut RuleCtx<Self>, _go: bool) {
                set!(ctx.a, *ctx.a + bits::<8>(1));
            }

            #[rule(conflict_free = "bump_a")]
            fn bump_b(ctx: &mut RuleCtx<Self>, _go: bool) {
                set!(ctx.b, *ctx.b + bits::<8>(1));
            }

            #[output]
            fn output(self_q: &Self, _go: bool) -> (Bits<8>, Bits<8>) {
                (*self_q.a, *self_q.b)
            }
        }
    }
}

#[test]
fn conflict_free_annotation_compiles_and_both_rules_fire() {
    use two_independent_counters::TwoIndependentCounters;
    let uut: TwoIndependentCounters = TwoIndependentCounters::default();
    let stream = std::iter::repeat_n(true, 5)
        .with_reset(2)
        .clock_pos_edge(100);
    let final_state = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output)
        .unwrap_or((bits(0), bits(0)));
    let (a, b) = (final_state.0.raw(), final_state.1.raw());
    // Both rules always fire (conflict-free, so no priority
    // suppression).  After 5 cycles each should have incremented
    // ~5 times.
    assert!(
        a >= 4 && a <= 5,
        "expected `a` to count up freely (no conflict suppression); got {a}",
    );
    assert!(
        b >= 4 && b <= 5,
        "expected `b` to count up freely (no conflict suppression); got {b}",
    );
}
