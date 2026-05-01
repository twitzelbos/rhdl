//! Phase 2 — negative tests for `urgent_before`.
//!
//! Same shape as `conflict_free_violation.rs`: invoke
//! `expand_rule_kernel` directly so we can match against the
//! diagnostic string instead of standing up a `trybuild` harness.

use proc_macro2::TokenStream;
use quote::quote;
use rhdl_rule_core::expand_rule_kernel;

#[test]
fn urgent_before_unknown_rule_rejected() {
    let input: TokenStream = quote! {
        pub struct Bad {
            val: dff::DFF<Bits<8>>,
        }

        impl Bad {
            #[rule(urgent_before = "nonexistent_rule")]
            fn r1(ctx: &mut RuleCtx<Self>, _flag: bool) {
                set!(ctx.val, bits::<8>(1));
            }

            #[output]
            fn output(self_q: &Self, _flag: bool) -> Bits<8> {
                *self_q.val
            }
        }
    };
    let result = expand_rule_kernel(input);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("urgent_before") && err.contains("unknown"),
        "expected diagnostic to mention urgent_before + unknown; got: {err}",
    );
}

#[test]
fn urgent_before_self_loop_rejected() {
    let input: TokenStream = quote! {
        pub struct Bad {
            val: dff::DFF<Bits<8>>,
        }

        impl Bad {
            #[rule(urgent_before = "r1")]
            fn r1(ctx: &mut RuleCtx<Self>, _flag: bool) {
                set!(ctx.val, bits::<8>(1));
            }

            #[output]
            fn output(self_q: &Self, _flag: bool) -> Bits<8> {
                *self_q.val
            }
        }
    };
    let result = expand_rule_kernel(input);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("urgent_before") && err.contains("itself"),
        "expected diagnostic to mention urgent_before + itself; got: {err}",
    );
}

#[test]
fn urgent_before_cycle_rejected() {
    // r1 urgent_before r2; r2 urgent_before r1.  Both write `val`,
    // so the cycle is between conflicting rules.
    let input: TokenStream = quote! {
        pub struct Bad {
            val: dff::DFF<Bits<8>>,
        }

        impl Bad {
            #[rule(urgent_before = "r2")]
            fn r1(ctx: &mut RuleCtx<Self>, _flag: bool) {
                set!(ctx.val, bits::<8>(1));
            }

            #[rule(urgent_before = "r1")]
            fn r2(ctx: &mut RuleCtx<Self>, _flag: bool) {
                set!(ctx.val, bits::<8>(2));
            }

            #[output]
            fn output(self_q: &Self, _flag: bool) -> Bits<8> {
                *self_q.val
            }
        }
    };
    let result = expand_rule_kernel(input);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("cycle"),
        "expected diagnostic to mention cycle; got: {err}",
    );
}

#[test]
fn urgent_before_meaningless_for_non_conflicting_pair_rejected() {
    // r1 writes `a`; r2 writes `b`.  Disjoint write sets, no
    // conflict, so urgent_before has no schedule effect.  The macro
    // should reject this so the user notices.
    let input: TokenStream = quote! {
        pub struct Bad {
            a: dff::DFF<Bits<8>>,
            b: dff::DFF<Bits<8>>,
        }

        impl Bad {
            #[rule(urgent_before = "r2")]
            fn r1(ctx: &mut RuleCtx<Self>, _flag: bool) {
                set!(ctx.a, bits::<8>(1));
            }

            #[rule]
            fn r2(ctx: &mut RuleCtx<Self>, _flag: bool) {
                set!(ctx.b, bits::<8>(2));
            }

            #[output]
            fn output(self_q: &Self, _flag: bool) -> Bits<8> {
                *self_q.a
            }
        }
    };
    let result = expand_rule_kernel(input);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("meaningless") && err.contains("don't conflict"),
        "expected diagnostic to mention meaningless + non-conflicting; got: {err}",
    );
}
