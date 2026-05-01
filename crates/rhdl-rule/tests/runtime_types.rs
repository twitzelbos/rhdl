//! Phase 2 — runtime ergonomic types.
//!
//! Verifies that the user-facing aliases from `rhdl-rule-rt` work
//! end-to-end inside a `rule_kernel!` invocation:
//!
//! - `Reg<T>` substitutes for the underlying `dff::DFF<T>` in field
//!   declarations.
//! - `RuleCtx<W>` is the recognised first-parameter type for
//!   `#[rule]` methods.
//!
//! Neither alias changes the emitted hardware — the macro
//! recognises them syntactically and the lowering is identical to
//! the canonical `dff::DFF<T>` form used in the older tests.

use rhdl::prelude::*;
use rhdl_rule::rule_kernel;
use rhdl_rule_rt::Reg;

rule_kernel! {
    pub struct AliasedCounter {
        // Reg<T> — the user-facing spelling for `dff::DFF<T>`.
        count: Reg<Bits<8>>,
    }

    impl AliasedCounter {
        #[rule]
        fn bump(ctx: &mut RuleCtx<Self>, enable: bool) {
            guard!(enable);
            set!(ctx.count, *ctx.count + bits::<8>(1));
        }

        #[output]
        fn output(self_q: &Self, _enable: bool) -> Bits<8> {
            *self_q.count
        }
    }
}

#[test]
fn reg_alias_resolves_to_dff_and_counter_works() {
    let uut: AliasedCounter = AliasedCounter::default();
    let stream = std::iter::repeat_n(true, 5)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    assert!(last >= 4 && last <= 5, "expected ~5 counts, got {last}");
}

#[test]
fn reg_alias_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: AliasedCounter = AliasedCounter::default();
    let stream = std::iter::repeat_n(true, 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    let tm = test_bench.ntl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}

#[test]
fn rule_ctx_marker_is_constructible_for_test_scaffolding() {
    // The `RuleCtx<W>` marker is normally invisible — the macro
    // strips it during expansion.  But it is `Default`-constructible
    // so hand-written tests that exercise the rule body directly can
    // stand one up.  (No such call site exists in shipped code yet;
    // this assertion exists to guarantee the API stays accessible.)
    let _ctx = rhdl_rule_rt::RuleCtx::<AliasedCounter>::new();
    let _ctx_default = rhdl_rule_rt::RuleCtx::<AliasedCounter>::default();
}
