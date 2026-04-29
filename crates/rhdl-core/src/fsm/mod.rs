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

/// Compile an FSM-tagged widget's kernel through Stage 1 and run
/// the canonical-transition extractor against the resulting RHIF.
///
/// This is the no-manual-list path called for by Phase 3 of
/// `fsm-architecture.md`: a widget that opts into the FSM derives
/// gets its transition graph derived directly from the kernel —
/// no author-curated `FSM_TRANSITIONS` const required, no
/// drift between the declared graph and what the kernel actually
/// implements.
///
/// Returns the [`extraction::ExtractionResult`] which bundles the
/// derived transitions with any per-arm `unanalyzable` diagnostics.
/// Callers that want a strict guarantee (no `Unanalyzable`
/// diagnostics permitted) should check `result.unanalyzable.is_empty()`
/// before consuming `result.transitions`.
pub fn extract_widget_transitions<W>() -> Result<extraction::ExtractionResult, crate::RHDLError>
where
    W: state::FsmWidget + crate::circuit::synchronous::SynchronousIO,
{
    let object = crate::compiler::driver::compile_design_stage1::<W::Kernel>(
        crate::CompilationMode::Synchronous,
    )?;
    let ops: Vec<crate::rhif::spec::OpCode> =
        object.ops.iter().map(|loc| loc.op.clone()).collect();
    let lookup = |slot: crate::rhif::spec::Slot| -> Option<crate::types::typed_bits::TypedBits> {
        slot.lit().map(|lit_id| object.symtab[lit_id].clone())
    };
    let desc = W::fsm_descriptor();
    Ok(extraction::extract_canonical_transitions(
        &ops,
        object.return_slot,
        &desc,
        &lookup,
    ))
}

/// Strict variant of [`extract_widget_transitions`]: returns the
/// derived transitions directly (sorted), but errors out on any
/// `Unanalyzable` diagnostic instead of bundling them.
///
/// Use this when you want a single-call "give me the transitions
/// or fail loudly" semantic — typically inside diagram-emission
/// helpers and drift-check tests.
pub fn extract_widget_transitions_strict<W>() -> Result<Vec<analysis::Transition>, crate::RHDLError>
where
    W: state::FsmWidget + crate::circuit::synchronous::SynchronousIO,
{
    let result = extract_widget_transitions::<W>()?;
    if !result.unanalyzable.is_empty() {
        return Err(anyhow::anyhow!(
            "FSM-transition extraction produced {} Unanalyzable diagnostic(s) for the kernel — \
             the kernel's match-on-state shape isn't fully recognisable to the canonical extractor: {:?}",
            result.unanalyzable.len(),
            result.unanalyzable,
        )
        .into());
    }
    let mut transitions = result.transitions;
    transitions.sort();
    Ok(transitions)
}
#[doc(inline)]
pub use state::{FsmState, FsmVariantDescriptor, FsmWidget};
