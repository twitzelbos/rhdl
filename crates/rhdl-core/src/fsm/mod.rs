//! Finite-state-machine metadata, static analysis, and diagram generation.
//!
//! This module is the runtime substrate for the
//! `#[derive(Fsm)]` / `#[fsm(...)]` / `#[fsm_kernel(...)]` macro
//! family.  The macros emit the metadata declared here; the
//! analysis passes and diagram renderers consume it.
//!
//! See `fsm-architecture.md` for the design plan.

pub mod analysis;
pub mod descriptor;
pub mod diagram;
pub mod extraction;
pub mod property;
pub mod state;

#[doc(inline)]
pub use analysis::{FsmDiagnostic, FsmDiagnosticKind, analyze_fsm_structure};
#[doc(inline)]
pub use descriptor::{FsmDescriptor, FsmKernelTag, FsmWidgetTag};
#[doc(inline)]
pub use diagram::{FsmDiagram, FsmDiagramJson, render_fsm_dot, render_fsm_svg};
#[doc(inline)]
pub use extraction::{ExtractionResult, LiteralLookup, extract_canonical_transitions};
#[doc(inline)]
pub use property::{FsmKernelProperties, FsmProperty, FsmPropertyKind, render_property_sva};

/// Top-level convenience: analyze an FSM-tagged kernel given a
/// pre-extracted transition set.  Combines the leaf analysis
/// from [`analysis`] with the descriptor lookup, returning the
/// full set of structural diagnostics.
pub fn analyze_fsm<W: state::FsmWidget>(
    transitions: &[analysis::Transition],
    unanalyzable: &[(&'static str, &'static str)],
) -> Vec<analysis::FsmDiagnostic> {
    let desc = W::fsm_descriptor();
    analysis::analyze_fsm_structure(&desc, transitions, unanalyzable)
}
#[doc(inline)]
pub use state::{FsmState, FsmVariantDescriptor, FsmWidget};
