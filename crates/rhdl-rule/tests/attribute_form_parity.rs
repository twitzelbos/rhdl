//! Token-level parity test: the function-like `rule_kernel!` and
//! the `#[rule_kernel_attr]` attribute MUST emit byte-identical
//! kernel + SynchronousIO impl for the same impl block.
//!
//! Both forms share `lower_rule_kernel` in `rhdl-rule-core`; this
//! test guards against accidental divergence if someone refactors
//! one path without the other.
//!
//! The function-like form additionally emits the augmented struct
//! (with derives injected); the attribute form emits only the
//! lowered impl + kernel + SynchronousIO.  The test strips the
//! struct portion from the function-like output before comparing.

use proc_macro2::TokenStream;
use quote::quote;
use rhdl_rule_core::{expand_rule_kernel, expand_rule_kernel_attr};

#[test]
fn function_like_and_attribute_emit_equivalent_kernels() {
    // Same impl block, two invocations.

    let function_like_input: TokenStream = quote! {
        pub struct ParityCounter {
            count: dff::DFF<Bits<8>>,
        }

        impl ParityCounter {
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
    };

    let attribute_input: TokenStream = quote! {
        impl ParityCounter {
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
    };

    let function_like_output = expand_rule_kernel(function_like_input)
        .expect("function-like form expands cleanly")
        .to_string();
    let attribute_output = expand_rule_kernel_attr(attribute_input)
        .expect("attribute form expands cleanly")
        .to_string();

    // The attribute output is a strict suffix of the function-like
    // output (the function-like form additionally prepends the
    // augmented struct definition).  Find where the lowered kernel
    // body begins by anchoring on `SynchronousIO` (the first thing
    // `lower_rule_kernel` emits) and slice from there.
    let pivot = function_like_output
        .find("SynchronousIO")
        .expect("function-like form contains the SynchronousIO impl");
    // Walk back to the `impl` keyword that starts the SynchronousIO
    // impl line — the pivot string is in the middle of a path.
    let impl_pivot = function_like_output[..pivot]
        .rfind("impl")
        .expect("`impl` keyword precedes the SynchronousIO path");
    let function_like_kernel_part = &function_like_output[impl_pivot..];

    assert_eq!(
        function_like_kernel_part, attribute_output,
        "function-like and attribute forms must emit byte-identical \
         kernel + SynchronousIO impl from the same impl block",
    );
}

#[test]
fn attribute_form_does_not_emit_struct_definition() {
    let input: TokenStream = quote! {
        impl SomeWidget {
            #[rule]
            fn r(ctx: &mut RuleCtx<Self>, _i: bool) {
                set!(ctx.x, bits::<8>(1));
            }

            #[output]
            fn output(self_q: &Self, _i: bool) -> Bits<8> {
                *self_q.x
            }
        }
    };

    let s = expand_rule_kernel_attr(input)
        .expect("attribute form expands cleanly")
        .to_string();

    // The attribute form must NOT emit a struct definition — that's
    // the user's responsibility.  Look for a `pub struct SomeWidget`
    // anywhere in the output as a negative check.
    assert!(
        !s.contains("pub struct SomeWidget") && !s.contains("struct SomeWidget"),
        "attribute form emitted a struct definition; it should leave \
         the struct to the user.  Output:\n{s}",
    );
}
