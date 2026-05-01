//! Token-level verification that the `mutually_exclusive` annotation
//! actually elides the suppressor term in the priority chain.
//!
//! Without the annotation, the second rule's `_fire_*` line in the
//! emitted kernel must include `!(_fire_<higher>)` to enforce the
//! priority-chain ordering.  With the annotation, that suppressor
//! must be absent — the user has asserted the guards are jointly
//! unsatisfiable, so the suppression is redundant.
//!
//! This test exercises [`expand_rule_kernel`] directly and inspects
//! the resulting token stream as a string.  It's the only way to
//! verify the optimisation without a Verilog-level diff (which
//! depends on the rest of the lowering pipeline).

use proc_macro2::TokenStream;
use quote::quote;
use rhdl_rule_core::expand_rule_kernel;

fn expansion(input: TokenStream) -> String {
    expand_rule_kernel(input).expect("expansion succeeds").to_string()
}

#[test]
fn without_mutually_exclusive_suppressor_is_present() {
    let input: TokenStream = quote! {
        pub struct TwoWriters {
            val: dff::DFF<Bits<8>>,
        }

        impl TwoWriters {
            #[rule(priority = 0)]
            fn high_writer(ctx: &mut RuleCtx<Self>, _flag: bool) {
                set!(ctx.val, bits::<8>(1));
            }

            #[rule(priority = 1)]
            fn low_writer(ctx: &mut RuleCtx<Self>, _flag: bool) {
                set!(ctx.val, bits::<8>(2));
            }

            #[output]
            fn output(self_q: &Self, _flag: bool) -> Bits<8> {
                *self_q.val
            }
        }
    };
    let s = expansion(input);
    // The suppressor MUST be present: low_writer is suppressed by
    // _fire_high_writer because both write `val`.
    assert!(
        s.contains("_fire_low_writer") && s.contains("! (_fire_high_writer)"),
        "expected priority chain to include `! (_fire_high_writer)` suppressor; got:\n{s}",
    );
}

#[test]
fn with_mutually_exclusive_suppressor_is_elided() {
    let input: TokenStream = quote! {
        pub struct TwoWriters {
            val: dff::DFF<Bits<8>>,
        }

        impl TwoWriters {
            #[rule(priority = 0)]
            fn high_writer(ctx: &mut RuleCtx<Self>, _flag: bool) {
                set!(ctx.val, bits::<8>(1));
            }

            #[rule(priority = 1, mutually_exclusive = "high_writer")]
            fn low_writer(ctx: &mut RuleCtx<Self>, _flag: bool) {
                set!(ctx.val, bits::<8>(2));
            }

            #[output]
            fn output(self_q: &Self, _flag: bool) -> Bits<8> {
                *self_q.val
            }
        }
    };
    let s = expansion(input);
    // The low_writer's `_fire_low_writer` line must NOT mention
    // `_fire_high_writer` — the assertion makes the suppressor
    // redundant and the optimisation drops it.
    let low_fire_position = s.find("let _fire_low_writer").expect("low_writer fire line");
    let after = &s[low_fire_position..];
    let semi = after.find(';').expect("statement terminator");
    let fire_stmt = &after[..semi];
    assert!(
        !fire_stmt.contains("_fire_high_writer"),
        "expected suppressor to be elided when mutually_exclusive is asserted; got:\n  {fire_stmt}",
    );
}

#[test]
fn mutually_exclusive_is_symmetric_either_side_works() {
    // Declare it on the OTHER side: high_writer says it's mutually
    // exclusive with low_writer.  The optimisation should still
    // apply when emitting low_writer's suppressor.
    let input: TokenStream = quote! {
        pub struct TwoWriters {
            val: dff::DFF<Bits<8>>,
        }

        impl TwoWriters {
            #[rule(priority = 0, mutually_exclusive = "low_writer")]
            fn high_writer(ctx: &mut RuleCtx<Self>, _flag: bool) {
                set!(ctx.val, bits::<8>(1));
            }

            #[rule(priority = 1)]
            fn low_writer(ctx: &mut RuleCtx<Self>, _flag: bool) {
                set!(ctx.val, bits::<8>(2));
            }

            #[output]
            fn output(self_q: &Self, _flag: bool) -> Bits<8> {
                *self_q.val
            }
        }
    };
    let s = expansion(input);
    let low_fire_position = s.find("let _fire_low_writer").expect("low_writer fire line");
    let after = &s[low_fire_position..];
    let semi = after.find(';').expect("statement terminator");
    let fire_stmt = &after[..semi];
    assert!(
        !fire_stmt.contains("_fire_high_writer"),
        "mutually_exclusive should be symmetric — declaring it on either side should elide \
         the suppressor; got:\n  {fire_stmt}",
    );
}
