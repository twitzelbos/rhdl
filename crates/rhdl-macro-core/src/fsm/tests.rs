use expect_test::expect_file;
use quote::quote;

use super::derive_fsm;

#[test]
fn fsm_derive_minimal_enum() {
    let input = quote! {
        pub enum State {
            #[default]
            Idle,
            Running { counter: b8 },
            Done,
        }
    };
    let output = derive_fsm(input).unwrap().to_string();
    let expected = expect_file!["expect/fsm_derive_minimal.expect"];
    expected.assert_eq(&output);
}

#[test]
fn fsm_derive_with_terminal_and_label() {
    let input = quote! {
        pub enum State {
            #[default]
            Idle,
            #[fsm_state(label = "running, counter = {counter}")]
            Running { counter: b8 },
            #[fsm_state(label = "complete", terminal)]
            Done,
        }
    };
    let output = derive_fsm(input).unwrap().to_string();
    let expected = expect_file!["expect/fsm_derive_with_decoration.expect"];
    expected.assert_eq(&output);
}

#[test]
fn fsm_derive_with_explicit_initial() {
    let input = quote! {
        #[fsm(initial = "Running")]
        pub enum State {
            Idle,
            Running,
            Done,
        }
    };
    let output = derive_fsm(input).unwrap().to_string();
    let expected = expect_file!["expect/fsm_derive_with_explicit_initial.expect"];
    expected.assert_eq(&output);
}

#[test]
fn fsm_derive_rejects_invalid_initial() {
    let input = quote! {
        #[fsm(initial = "DoesNotExist")]
        pub enum State {
            Idle,
            Running,
        }
    };
    let err = derive_fsm(input).unwrap_err().to_string();
    assert!(
        err.contains("does not exist"),
        "expected `does not exist` in error, got: {err}"
    );
}

#[test]
fn fsm_derive_rejects_struct() {
    let input = quote! {
        pub struct NotAnEnum {
            x: u8,
        }
    };
    let err = derive_fsm(input).unwrap_err().to_string();
    assert!(
        err.contains("only supports enums"),
        "expected `only supports enums` in error, got: {err}"
    );
}

#[test]
fn fsm_derive_rejects_unknown_state_attr() {
    let input = quote! {
        pub enum State {
            Idle,
            #[fsm_state(typo_keyword)]
            Boom,
        }
    };
    let err = derive_fsm(input).unwrap_err().to_string();
    assert!(
        err.contains("unrecognised fsm_state attribute"),
        "expected `unrecognised fsm_state attribute` in error, got: {err}"
    );
}
