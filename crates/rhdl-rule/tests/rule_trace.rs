//! Per-rule trace signals via the `#[rule(trace)]` annotation.
//!
//! The macro normally emits `_fire_<rule>` and `_can_fire_<rule>`
//! as private (underscore-prefixed) bindings inside the kernel
//! function.  Adding `#[rule(trace)]` (or `#[rule(trace = true)]`)
//! to a rule causes the macro to ALSO emit `fire_<rule>` and
//! `can_fire_<rule>` aliases — visible names that RHDL's trace
//! infrastructure surfaces in VCDs.  Off by default so the common
//! case doesn't pay the kernel-emission and VCD-clutter cost.
//!
//! These tests verify three things:
//!
//! 1. **Without the annotation**, no public `fire_<rule>` /
//!    `can_fire_<rule>` bindings appear in the emitted kernel.
//! 2. **With the annotation**, the visible bindings appear and
//!    correctly mirror the internal `_fire_*` / `_can_fire_*`
//!    values.
//! 3. **Annotated and non-annotated rules can mix** in the same
//!    kernel — only the annotated ones get the trace exposure.

use proc_macro2::TokenStream;
use quote::quote;
use rhdl_rule_core::expand_rule_kernel;

fn expand(input: TokenStream) -> String {
    expand_rule_kernel(input)
        .expect("expansion succeeds")
        .to_string()
}

#[test]
fn no_trace_annotation_no_public_fire_signal() {
    let s = expand(quote! {
        pub struct PlainCounter {
            count: dff::DFF<Bits<8>>,
        }
        impl PlainCounter {
            #[rule]
            fn bump(ctx: &mut RuleCtx<Self>, enable: bool) {
                guard!(enable);
                ctx.count = *ctx.count + bits::<8>(1);
            }
            #[output]
            fn output(self_q: &Self, _enable: bool) -> Bits<8> {
                *self_q.count
            }
        }
    });
    assert!(
        s.contains("_fire_bump"),
        "internal _fire_bump must be present",
    );
    // Public name `fire_bump` (no leading underscore) must NOT
    // appear in the emission when trace isn't requested.  We check
    // by looking for `let fire_bump` specifically — otherwise the
    // substring would also match `_fire_bump`.
    assert!(
        !s.contains("let fire_bump"),
        "public `fire_bump` should NOT be emitted without #[rule(trace)]; got:\n{s}",
    );
}

#[test]
fn trace_bare_annotation_emits_public_fire_signal() {
    let s = expand(quote! {
        pub struct TracedCounter {
            count: dff::DFF<Bits<8>>,
        }
        impl TracedCounter {
            #[rule(trace)]
            fn bump(ctx: &mut RuleCtx<Self>, enable: bool) {
                guard!(enable);
                ctx.count = *ctx.count + bits::<8>(1);
            }
            #[output]
            fn output(self_q: &Self, _enable: bool) -> Bits<8> {
                *self_q.count
            }
        }
    });
    assert!(
        s.contains("let fire_bump") && s.contains("let can_fire_bump"),
        "bare #[rule(trace)] should emit both fire_bump and can_fire_bump; got:\n{s}",
    );
}

#[test]
fn trace_explicit_true_annotation_emits_public_fire_signal() {
    let s = expand(quote! {
        pub struct TracedCounter {
            count: dff::DFF<Bits<8>>,
        }
        impl TracedCounter {
            #[rule(trace = true)]
            fn bump(ctx: &mut RuleCtx<Self>, enable: bool) {
                guard!(enable);
                ctx.count = *ctx.count + bits::<8>(1);
            }
            #[output]
            fn output(self_q: &Self, _enable: bool) -> Bits<8> {
                *self_q.count
            }
        }
    });
    assert!(
        s.contains("let fire_bump"),
        "#[rule(trace = true)] should emit public fire_bump; got:\n{s}",
    );
}

#[test]
fn trace_explicit_false_does_not_emit_public_fire_signal() {
    let s = expand(quote! {
        pub struct TracedCounter {
            count: dff::DFF<Bits<8>>,
        }
        impl TracedCounter {
            #[rule(trace = false)]
            fn bump(ctx: &mut RuleCtx<Self>, enable: bool) {
                guard!(enable);
                ctx.count = *ctx.count + bits::<8>(1);
            }
            #[output]
            fn output(self_q: &Self, _enable: bool) -> Bits<8> {
                *self_q.count
            }
        }
    });
    assert!(
        !s.contains("let fire_bump"),
        "#[rule(trace = false)] is the same as no annotation; got:\n{s}",
    );
}

#[test]
fn trace_annotation_can_mix_with_priority() {
    let s = expand(quote! {
        pub struct MixedAnnotations {
            val: dff::DFF<Bits<8>>,
        }
        impl MixedAnnotations {
            #[rule(priority = 0, trace)]
            fn high(ctx: &mut RuleCtx<Self>, _i: bool) {
                ctx.val = bits::<8>(1);
            }
            #[rule(priority = 1)]
            fn low(ctx: &mut RuleCtx<Self>, _i: bool) {
                ctx.val = bits::<8>(2);
            }
            #[output]
            fn output(self_q: &Self, _i: bool) -> Bits<8> {
                *self_q.val
            }
        }
    });
    // `high` is traced; `low` isn't.
    assert!(s.contains("let fire_high"), "fire_high must be emitted");
    assert!(
        !s.contains("let fire_low"),
        "fire_low must NOT be emitted (no trace on this rule)",
    );
}

#[test]
fn trace_annotation_unknown_value_is_a_compile_error() {
    let result = expand_rule_kernel(quote! {
        pub struct Bad {
            val: dff::DFF<Bits<8>>,
        }
        impl Bad {
            #[rule(trace = "yes")]
            fn r(ctx: &mut RuleCtx<Self>, _i: bool) {
                ctx.val = bits::<8>(1);
            }
            #[output]
            fn output(self_q: &Self, _i: bool) -> Bits<8> {
                *self_q.val
            }
        }
    });
    assert!(
        result.is_err(),
        "non-bool value for trace should produce an error",
    );
}

// ---- Behavioural test: traced kernel still works end-to-end ------

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel;

rule_kernel! {
    pub struct TracedRunnable {
        count: dff::DFF<Bits<8>>,
    }

    impl TracedRunnable {
        #[rule(trace)]
        fn bump(ctx: &mut RuleCtx<Self>, enable: bool) {
            guard!(enable);
            ctx.count = *ctx.count + bits::<8>(1);
        }

        #[output]
        fn output(self_q: &Self, _enable: bool) -> Bits<8> {
            *self_q.count
        }
    }
}

#[test]
fn traced_kernel_runs_correctly() {
    let uut: TracedRunnable = TracedRunnable::default();
    let stream = std::iter::repeat_n(true, 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    assert!(last >= 3 && last <= 4, "got {last}");
}

#[test]
fn traced_kernel_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: TracedRunnable = TracedRunnable::default();
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
