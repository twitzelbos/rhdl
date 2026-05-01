//! Compile-fail-style negative test: the `conflict_free`
//! assertion is rejected when the computed conflict matrix says
//! the named pair *does* conflict.
//!
//! We test this via the `expand_rule_kernel` API directly (so we
//! can inspect the diagnostic string instead of relying on
//! `trybuild` infrastructure for one test).

use proc_macro2::TokenStream;
use quote::quote;
use rhdl_rule_core::expand_rule_kernel;

#[test]
fn conflict_free_assertion_rejected_when_pair_conflicts() {
    // Two rules that BOTH write `val` (write-write conflict) but
    // claim conflict-freedom.  The macro must reject this.
    let input: TokenStream = quote! {
        pub struct Bad {
            val: dff::DFF<Bits<8>>,
        }

        impl Bad {
            #[rule(conflict_free = "set_high")]
            fn set_low(ctx: &mut RuleCtx<Self>, _flag: bool) {
                set!(ctx.val, bits::<8>(7));
            }

            #[rule]
            fn set_high(ctx: &mut RuleCtx<Self>, _flag: bool) {
                set!(ctx.val, bits::<8>(99));
            }

            #[output]
            fn output(self_q: &Self, _flag: bool) -> Bits<8> {
                *self_q.val
            }
        }
    };
    let result = expand_rule_kernel(input);
    assert!(
        result.is_err(),
        "expected expand_rule_kernel to reject the conflict_free assertion",
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("conflict_free") && err.contains("set_high"),
        "expected diagnostic to mention conflict_free and the named rule; got: {err}",
    );
}

#[test]
fn conflict_free_assertion_referencing_unknown_rule_rejected() {
    let input: TokenStream = quote! {
        pub struct Bad {
            val: dff::DFF<Bits<8>>,
        }

        impl Bad {
            #[rule(conflict_free = "nonexistent_rule")]
            fn the_only_rule(ctx: &mut RuleCtx<Self>, _flag: bool) {
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
        err.contains("nonexistent_rule") && err.contains("unknown"),
        "expected diagnostic to mention the unknown rule name; got: {err}",
    );
}
