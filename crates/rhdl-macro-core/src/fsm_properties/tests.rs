use expect_test::expect_file;
use quote::quote;

use super::fsm_properties;

#[test]
fn empty_properties_emit_zero_length_table() {
    let attr = quote! {};
    let item = quote! {
        pub fn my_kernel() -> () {}
    };
    let output = fsm_properties(attr, item).unwrap().to_string();
    let expected = expect_file!["expect/fsm_properties_empty.expect"];
    expected.assert_eq(&output);
}

#[test]
fn single_invariant_emits_one_row() {
    let attr = quote! {
        invariant("state != State::Error", name = "no_error")
    };
    let item = quote! {
        pub fn my_kernel() -> () {}
    };
    let output = fsm_properties(attr, item).unwrap().to_string();
    let expected = expect_file!["expect/fsm_properties_invariant.expect"];
    expected.assert_eq(&output);
}

#[test]
fn multiple_kinds_emit_in_order() {
    let attr = quote! {
        invariant("state != State::Error"),
        cover("state == State::Done"),
        liveness("state == State::Done", bound = 1024),
        assume("input.valid"),
    };
    let item = quote! {
        pub fn my_kernel() -> () {}
    };
    let output = fsm_properties(attr, item).unwrap().to_string();
    let expected = expect_file!["expect/fsm_properties_all_kinds.expect"];
    expected.assert_eq(&output);
}

#[test]
fn rejects_unknown_kind() {
    let attr = quote! { typo_kind("expr") };
    let item = quote! { pub fn my_kernel() -> () {} };
    let err = fsm_properties(attr, item).unwrap_err().to_string();
    assert!(
        err.contains("unknown fsm property kind"),
        "expected `unknown fsm property kind` in: {err}"
    );
}

#[test]
fn rejects_non_string_expression() {
    let attr = quote! { invariant(42) };
    let item = quote! { pub fn my_kernel() -> () {} };
    let err = fsm_properties(attr, item).unwrap_err().to_string();
    assert!(
        err.contains("expression argument must be a string literal"),
        "expected expression-error in: {err}"
    );
}

#[test]
fn rejects_unknown_named_arg() {
    let attr = quote! { invariant("expr", typo_arg = "value") };
    let item = quote! { pub fn my_kernel() -> () {} };
    let err = fsm_properties(attr, item).unwrap_err().to_string();
    assert!(
        err.contains("must be `name` or `bound`"),
        "expected named-arg-error in: {err}"
    );
}

#[test]
fn requires_at_least_one_positional_arg() {
    let attr = quote! { invariant() };
    let item = quote! { pub fn my_kernel() -> () {} };
    let err = fsm_properties(attr, item).unwrap_err().to_string();
    assert!(
        err.contains("requires at least one positional argument"),
        "expected positional-arg-error in: {err}"
    );
}

#[test]
fn auto_names_properties_without_explicit_name() {
    let attr = quote! {
        invariant("a == b"),
        invariant("c == d"),
    };
    let item = quote! { pub fn my_kernel() -> () {} };
    let output = fsm_properties(attr, item).unwrap().to_string();
    assert!(output.contains("my_kernel_prop_0"));
    assert!(output.contains("my_kernel_prop_1"));
}
