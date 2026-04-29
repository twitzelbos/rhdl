//! The `FsmState` trait and its variant-descriptor type.
//!
//! These are the metadata surface that `#[derive(Fsm)]` lights up
//! on a `Digital`-derived enum.  Both are pure compile-time
//! reflection — there is no runtime cost beyond a static slice.

use crate::types::digital::Digital;

/// Per-variant metadata for a state enum derived as an FSM.
///
/// One of these is emitted into a `&'static [FsmVariantDescriptor]`
/// for every enum that `#[derive(Fsm)]` is applied to.  The slice
/// is what the static-analysis pass and the diagram generator
/// walk; the data is kept primitive so the slice is `const`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsmVariantDescriptor {
    /// The variant's name as written in the source enum (no
    /// path prefix; just the bare identifier — `Idle`, not
    /// `State::Idle`).
    pub name: &'static str,
    /// The variant's discriminant value, normalised to `i128` so
    /// both `#[repr(u32)]`-style and signed-discriminant enums fit.
    pub discriminant: i128,
    /// True if the variant carries a struct-payload (`Foo { ... }`)
    /// or tuple payload (`Foo(...)`).  Pure unit variants are `false`.
    pub has_payload: bool,
    /// True if the variant was annotated `#[fsm_state(terminal)]`.
    /// The static-analysis pass treats terminal variants as
    /// intentionally absorbing — i.e., the lack of an outgoing
    /// transition is not a deadlock candidate.
    pub terminal: bool,
    /// Optional display label for diagram rendering.  When `None`,
    /// the diagram renderer falls back to the variant name.
    pub label: Option<&'static str>,
}

/// Marker trait emitted by `#[derive(Fsm)]`.
///
/// An FSM-state type is always also a `Digital` enum (the derive
/// itself does not impl `Digital` — that's the orthogonal
/// `#[derive(Digital)]` macro's job; `#[derive(Fsm)]` adds the
/// metadata layer on top).  The trait carries enough information
/// for the static-analysis pass to walk transitions and for the
/// diagram generator to render nodes.
///
/// See `fsm-architecture.md` §4.2.
pub trait FsmState: Digital {
    /// The static slice of per-variant metadata, in source order.
    fn fsm_variants() -> &'static [FsmVariantDescriptor];

    /// The index into [`Self::fsm_variants`] of the FSM's initial
    /// variant.  Defaults to whatever variant the source enum's
    /// `#[default]` attribute marked, or 0 if none was supplied.
    fn fsm_initial_index() -> usize;

    /// The variant index of `self`.  Used by the analysis pass
    /// when stitching together transitions extracted from the
    /// kernel against the variant table.
    fn fsm_variant_index(&self) -> usize;
}

/// Convenience: look up a variant by name.  Returns `None` if no
/// variant of the given name exists in this FSM's metadata.
pub fn fsm_variant_index_by_name<S: FsmState>(name: &str) -> Option<usize> {
    S::fsm_variants().iter().position(|v| v.name == name)
}

/// Marker trait emitted by `#[derive(FsmWidget)]`.
///
/// A widget that has the FSM tooling lit up implements this
/// trait, exposing the widget's [`super::FsmDescriptor`] via the
/// `fsm_descriptor()` associated function.  The static-analysis
/// pass and the diagram generator look up the descriptor through
/// this trait, so they stay decoupled from the widget's concrete
/// state enum.
pub trait FsmWidget {
    /// The state-enum type that this widget's state DFF holds.
    type StateEnum: FsmState;

    /// The widget's compiled FSM descriptor (variant table +
    /// state-field tag + state-var tag).  Returns a fresh value
    /// each call — descriptors are `Copy` and cheap to recompute.
    fn fsm_descriptor() -> super::descriptor::FsmDescriptor;
}
