//! Compile-time tags for FSM-aware widgets and kernels.
//!
//! `FsmWidgetTag` is what `#[fsm(state_field = "...")]` records on
//! a `Synchronous`-derived struct: it names the field that holds
//! the state DFF.  `FsmKernelTag` is what `#[fsm_kernel(state_var
//! = "...")]` records on the kernel function: it names the local
//! expression the kernel matches against.
//!
//! Both tags are `&'static`-friendly; the macro emits them as
//! associated constants on a per-widget marker trait.  The
//! analysis pass and the diagram generator look up the tags by
//! the widget's type name during compilation.

/// Tag emitted by `#[fsm(state_field = "...")]` on a widget struct.
///
/// Records the name of the DFF-typed field that holds the FSM's
/// state.  The static-analysis pass uses this to find the state
/// register in the widget's `Synchronous` impl and locate the
/// corresponding entry in the kernel's `Q`/`D` aggregates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsmWidgetTag {
    /// Identifier of the state-bearing field on the widget struct
    /// (matches the field name written in source).
    pub state_field: &'static str,
    /// Whether the static-analysis pass should treat its
    /// diagnostics as errors rather than warnings, set by
    /// `#[fsm(strict)]`.
    pub strict: bool,
    /// Whether the extractor should include implicit self-loops in
    /// the transition graph, set by `#[fsm(allow_implicit)]`.
    ///
    /// When `true`: the canonical RHDL kernel pattern (kernel-top
    /// default `d.<state_field> = q.<state_field>` + selective
    /// override in arms that transition) is recognised; an arm that
    /// omits the `d.<state_field>` write produces a `(s, s)`
    /// self-loop edge.  This is the typical case for FSM widgets in
    /// the corpus.
    ///
    /// When `false` (default): the extractor only emits transitions
    /// for *explicit* writes to `d.<state_field>` inside an arm.
    /// Arms that fall through to the kernel-top default are NOT
    /// counted as transitions; states whose only outgoing edges
    /// would have been implicit self-loops appear with no outgoing
    /// edges, which causes the Layer 2 `DeadlockCandidate`
    /// diagnostic to fire.  This catches forgotten transitions —
    /// the structural correctness gap documented in
    /// `fsm-architecture.md` §5.4.1 (now closed by this opt-in).
    ///
    /// **Default is `false` (strict).**  Widgets that genuinely
    /// want stay-in-place via the canonical default must opt in.
    /// This makes deadlock detection useful by default; the opt-in
    /// makes the author's intent explicit when they do want
    /// implicit holds.
    pub allow_implicit: bool,
}

/// Tag emitted by `#[fsm_kernel(state_var = "...")]` on a kernel
/// function.
///
/// Records the local expression the kernel matches against to
/// drive the FSM transition logic.  Defaults to `q.<state_field>`
/// when no explicit `state_var` is supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsmKernelTag {
    /// The textual form of the state-binding expression as written
    /// in source — e.g., `"q.state"` for the canonical case.
    /// Stored as a string because the analysis pass parses it back
    /// as a path expression to look up the bound DFF.
    pub state_var: &'static str,
}

/// The combined descriptor a widget exposes through the
/// `FsmDescriptor`-implementing helper trait.
///
/// One `FsmDescriptor` per FSM-tagged widget; produced by gluing
/// the widget tag, the kernel tag, and a pointer to the static
/// variant table together.  Consumers (Layer 2, Layer 3, Layer 4)
/// read from this descriptor.
#[derive(Debug, Clone, Copy)]
pub struct FsmDescriptor {
    /// The widget's fully-qualified type name (used as a stable
    /// key in cross-pass tables and for diagnostic messages).
    pub widget_name: &'static str,
    /// The widget-side tag (state-field name + strict flag).
    pub widget: FsmWidgetTag,
    /// The kernel-side tag (state-var binding expression).
    pub kernel: FsmKernelTag,
    /// The state enum's static variant table.  The descriptor
    /// stores it as a function pointer so the descriptor itself
    /// can be `'static` without monomorphising on the enum type.
    pub variants_fn: fn() -> &'static [super::state::FsmVariantDescriptor],
    /// The state enum's initial-variant index (mirrors
    /// `FsmState::fsm_initial_index`).
    pub initial_fn: fn() -> usize,
}

impl FsmDescriptor {
    /// Convenience: fetch the variant table.
    pub fn variants(&self) -> &'static [super::state::FsmVariantDescriptor] {
        (self.variants_fn)()
    }

    /// Convenience: fetch the initial-variant index.
    pub fn initial_index(&self) -> usize {
        (self.initial_fn)()
    }
}
