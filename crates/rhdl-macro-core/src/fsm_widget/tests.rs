use expect_test::expect_file;
use quote::quote;

use super::derive_fsm_widget;

#[test]
fn fsm_widget_minimal() {
    let input = quote! {
        #[fsm(state_field = "state", state_enum = State)]
        pub struct Machine {
            state: dff::DFF<State>,
        }
    };
    let output = derive_fsm_widget(input).unwrap().to_string();
    let expected = expect_file!["expect/fsm_widget_minimal.expect"];
    expected.assert_eq(&output);
}

#[test]
fn fsm_widget_with_strict_flag() {
    let input = quote! {
        #[fsm(state_field = "state", state_enum = State, strict)]
        pub struct Machine {
            state: dff::DFF<State>,
        }
    };
    let output = derive_fsm_widget(input).unwrap().to_string();
    let expected = expect_file!["expect/fsm_widget_with_strict.expect"];
    expected.assert_eq(&output);
}

#[test]
fn fsm_widget_state_enum_as_string_literal() {
    // Both `state_enum = State` and `state_enum = "State"` should
    // work; the string-literal form is occasionally needed when
    // the path hasn't been imported into the surrounding scope.
    let input = quote! {
        #[fsm(state_field = "state", state_enum = "crate::states::State")]
        pub struct Machine {
            state: dff::DFF<crate::states::State>,
        }
    };
    let output = derive_fsm_widget(input).unwrap().to_string();
    let expected = expect_file!["expect/fsm_widget_state_enum_string.expect"];
    expected.assert_eq(&output);
}

#[test]
fn fsm_widget_rejects_missing_state_field() {
    let input = quote! {
        #[fsm(state_enum = State)]
        pub struct Machine {
            state: dff::DFF<State>,
        }
    };
    let err = derive_fsm_widget(input).unwrap_err().to_string();
    assert!(
        err.contains("requires `#[fsm(state_field"),
        "expected `requires #[fsm(state_field` in error, got: {err}",
    );
}

#[test]
fn fsm_widget_rejects_missing_state_enum() {
    let input = quote! {
        #[fsm(state_field = "state")]
        pub struct Machine {
            state: dff::DFF<State>,
        }
    };
    let err = derive_fsm_widget(input).unwrap_err().to_string();
    assert!(
        err.contains("requires `#[fsm(state_enum"),
        "expected `requires #[fsm(state_enum` in error, got: {err}",
    );
}

#[test]
fn fsm_widget_rejects_unknown_field() {
    let input = quote! {
        #[fsm(state_field = "not_a_real_field", state_enum = State)]
        pub struct Machine {
            state: dff::DFF<State>,
        }
    };
    let err = derive_fsm_widget(input).unwrap_err().to_string();
    assert!(
        err.contains("does not exist on"),
        "expected `does not exist on` in error, got: {err}",
    );
}

#[test]
fn fsm_widget_rejects_enum() {
    let input = quote! {
        pub enum NotAStruct { A, B }
    };
    let err = derive_fsm_widget(input).unwrap_err().to_string();
    assert!(
        err.contains("only supports structs"),
        "expected `only supports structs` in error, got: {err}",
    );
}

#[test]
fn fsm_widget_ignores_initial_attribute() {
    // The `#[fsm(initial = "...")]` attribute is meant for the
    // enum-side derive; the widget-side derive should silently
    // pass it through (since the same struct may carry the
    // attribute in some workflows).
    let input = quote! {
        #[fsm(state_field = "state", state_enum = State, initial = "Idle")]
        pub struct Machine {
            state: dff::DFF<State>,
        }
    };
    derive_fsm_widget(input).expect("initial = ... must not error on the widget side");
}
