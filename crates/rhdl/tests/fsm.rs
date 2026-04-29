//! Integration tests for the `#[derive(Fsm)]` macro family.
//!
//! These tests exercise the full path from source → proc-macro
//! expansion → trait impl → variant-table lookup.  They live as
//! integration tests (in `crates/rhdl/tests/`) rather than as
//! macro-snapshot tests because they verify the *runtime*
//! contract: the trait methods return what the source enum says
//! they should.

use rhdl::prelude::*;

#[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
pub enum Simple {
    #[default]
    Idle,
    Running,
    Done,
}

#[test]
fn simple_fsm_variant_table() {
    let variants = Simple::fsm_variants();
    assert_eq!(variants.len(), 3);
    assert_eq!(variants[0].name, "Idle");
    assert_eq!(variants[1].name, "Running");
    assert_eq!(variants[2].name, "Done");
    assert!(!variants[0].has_payload);
    assert!(!variants[0].terminal);
    assert!(variants[0].label.is_none());
}

#[test]
fn simple_fsm_initial_index_picks_default() {
    assert_eq!(Simple::fsm_initial_index(), 0);
}

#[test]
fn simple_fsm_variant_index_round_trip() {
    assert_eq!(Simple::Idle.fsm_variant_index(), 0);
    assert_eq!(Simple::Running.fsm_variant_index(), 1);
    assert_eq!(Simple::Done.fsm_variant_index(), 2);
}

#[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
#[fsm(initial = "Running")]
pub enum WithExplicitInitial {
    #[default]
    Idle,
    Running,
    Done,
}

#[test]
fn explicit_initial_overrides_default() {
    assert_eq!(WithExplicitInitial::fsm_initial_index(), 1);
}

#[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
pub enum WithDecoration {
    #[default]
    #[fsm_state(label = "idle, waiting for start")]
    Idle,
    #[fsm_state(label = "running")]
    Running { counter: b8 },
    #[fsm_state(label = "complete", terminal)]
    Done,
}

#[test]
fn decoration_lands_on_variant_table() {
    let v = WithDecoration::fsm_variants();
    assert_eq!(v[0].label, Some("idle, waiting for start"));
    assert!(v[1].has_payload);
    assert_eq!(v[2].label, Some("complete"));
    assert!(v[2].terminal);
    // Non-payload variants stay non-payload.
    assert!(!v[0].has_payload);
    assert!(!v[2].has_payload);
}

#[test]
fn lookup_by_name_works() {
    use rhdl_core::fsm::state::fsm_variant_index_by_name;
    assert_eq!(fsm_variant_index_by_name::<Simple>("Running"), Some(1));
    assert_eq!(fsm_variant_index_by_name::<Simple>("Boom"), None);
}

#[test]
fn variant_index_works_with_payload() {
    let s = WithDecoration::Running { counter: bits(42) };
    assert_eq!(s.fsm_variant_index(), 1);
}

// ---------------------------------------------------------------------------
// Layer 1b: widget-side `#[derive(FsmWidget)]` integration tests.
// ---------------------------------------------------------------------------

mod widget_layer {
    use super::Simple;
    use rhdl::prelude::*;

    /// A synthetic widget struct.  `state` is just a `Bits<2>` for
    /// the purposes of the test (the real widget pattern wraps the
    /// state DFF, but the FsmWidget derive only needs to know the
    /// field name + the state enum type — not the wrapper).
    #[derive(FsmWidget)]
    #[fsm(state_field = "state", state_enum = Simple)]
    pub struct Machine {
        #[allow(dead_code)]
        state: Bits<2>,
    }

    #[test]
    fn widget_descriptor_has_correct_widget_name() {
        let desc = Machine::fsm_descriptor();
        assert_eq!(desc.widget_name, "Machine");
        assert_eq!(desc.widget.state_field, "state");
        assert!(!desc.widget.strict);
        // Default `state_var` binding is `q.<state_field>`.
        assert_eq!(desc.kernel.state_var, "q.state");
    }

    #[test]
    fn widget_descriptor_round_trips_state_metadata() {
        let desc = Machine::fsm_descriptor();
        let variants = desc.variants();
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[0].name, "Idle");
        assert_eq!(desc.initial_index(), 0);
    }

    #[derive(FsmWidget)]
    #[fsm(state_field = "state", state_enum = Simple, strict)]
    pub struct StrictMachine {
        #[allow(dead_code)]
        state: Bits<2>,
    }

    #[test]
    fn strict_flag_is_recorded() {
        assert!(StrictMachine::fsm_descriptor().widget.strict);
    }

    // -----------------------------------------------------------
    // Layer 2: static analysis.
    // -----------------------------------------------------------

    #[test]
    fn analyze_fully_connected_fsm_has_no_diagnostics() {
        use rhdl_core::fsm::analysis::Transition;
        let diags = rhdl_core::fsm::analyze_fsm::<Machine>(
            &[
                Transition { source_index: 0, target_index: 1 },
                Transition { source_index: 1, target_index: 2 },
                Transition { source_index: 2, target_index: 0 },
            ],
            &[],
        );
        assert!(diags.is_empty(), "expected no diagnostics, got: {diags:#?}");
    }

    #[test]
    fn analyze_unreachable_state_via_widget_helper() {
        use rhdl_core::fsm::analysis::{FsmDiagnosticKind, Transition};
        let diags = rhdl_core::fsm::analyze_fsm::<Machine>(
            &[Transition { source_index: 0, target_index: 1 }],
            &[],
        );
        // Done is unreachable from Idle if Running has no outgoing edge.
        assert!(diags.iter().any(|d| matches!(
            d.kind,
            FsmDiagnosticKind::UnreachableState { name: "Done" }
        )));
    }
}

// -----------------------------------------------------------
// Layer 3: diagram rendering integration.
// -----------------------------------------------------------

mod diagram_layer {
    use super::Simple;
    use rhdl::prelude::*;

    #[derive(FsmWidget)]
    #[fsm(state_field = "state", state_enum = Simple)]
    pub struct DiagramMachine {
        #[allow(dead_code)]
        state: Bits<2>,
    }

    #[test]
    fn build_diagram_from_widget_descriptor() {
        use rhdl_core::fsm::analysis::Transition;
        use rhdl_core::fsm::diagram::{build_fsm_diagram, render_fsm_dot, render_fsm_json, render_fsm_svg};

        let desc = DiagramMachine::fsm_descriptor();
        let transitions = [
            Transition { source_index: 0, target_index: 1 },
            Transition { source_index: 1, target_index: 2 },
            Transition { source_index: 2, target_index: 0 },
        ];
        let diagram = build_fsm_diagram(&desc, &transitions);
        assert_eq!(diagram.widget_name, "DiagramMachine");
        assert_eq!(diagram.nodes.len(), 3);
        // SVG, dot, and JSON all produce non-empty strings.
        let svg = render_fsm_svg(&diagram);
        let dot = render_fsm_dot(&diagram);
        let json = render_fsm_json(&diagram);
        assert!(svg.starts_with("<svg"));
        assert!(dot.contains("digraph"));
        assert!(json.0.contains("DiagramMachine"));
    }
}

// -----------------------------------------------------------
// Layer 4: SVA properties via #[fsm_properties(...)].
// -----------------------------------------------------------

mod property_layer {
    use rhdl::prelude::*;

    #[fsm_properties(
        invariant("state != State::Error", name = "no_error"),
        cover("state == State::Done"),
        liveness("state == State::Done", bound = 1024),
        assume("input.valid"),
    )]
    pub fn my_kernel() {}

    #[test]
    fn properties_table_is_populated() {
        let props = <FsmProps_my_kernel as FsmKernelProperties>::fsm_properties();
        assert_eq!(props.len(), 4);
        assert_eq!(props[0].kind, FsmPropertyKind::Invariant);
        assert_eq!(props[0].name, "no_error");
        assert_eq!(props[1].kind, FsmPropertyKind::Cover);
        assert_eq!(props[1].name, "my_kernel_prop_1");
        assert_eq!(props[2].kind, FsmPropertyKind::Liveness);
        assert_eq!(props[2].bound, Some(1024));
        assert_eq!(props[3].kind, FsmPropertyKind::Assume);
    }

    #[test]
    fn render_property_sva_round_trip() {
        let props = <FsmProps_my_kernel as FsmKernelProperties>::fsm_properties();
        let sva = rhdl_core::fsm::render_property_sva(props);
        assert!(sva.contains("assert property (no_error_p)"));
        assert!(sva.contains("cover property (my_kernel_prop_1_p)"));
        assert!(sva.contains("##[1:1024]"));
        assert!(sva.contains("assume property"));
    }

    // Sanity: the kernel function itself still compiles + is callable.
    #[test]
    fn kernel_function_passes_through_unchanged() {
        my_kernel();
    }

    #[fsm_properties()]
    pub fn empty_kernel() {}

    #[test]
    fn empty_property_list_is_legal() {
        let props = <FsmProps_empty_kernel as FsmKernelProperties>::fsm_properties();
        assert_eq!(props.len(), 0);
    }
}
