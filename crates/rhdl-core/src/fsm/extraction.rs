//! Extract the FSM transition graph from a kernel's RHIF.
//!
//! Layer 2's analysis pass needs to know, for each FSM-tagged
//! kernel, the set of `(source variant, target variant)` pairs the
//! kernel's next-state function can produce.  This module is the
//! extractor: it computes that set by static data-flow analysis on
//! the kernel's RHIF, in the format that
//! [`super::analysis::analyze_fsm_structure`] consumes.
//!
//! ## Definition (per `fsm-architecture.md` §5.1)
//!
//! Given an RHDL kernel `K` with declared FSM state field
//! `<state_field>` of enum type `E`, the FSM transition graph
//! `G(K) ⊆ Variants(E) × Variants(E)` is the relation
//!
//! ```text
//! (s, t) ∈ G(K)  ⟺  ∃ input I such that
//!                       evaluating K under (q.<state_field> = s, other inputs = I)
//!                       produces  d.<state_field> = t
//! ```
//!
//! This definition is about the kernel's I/O (a pure function of
//! `(cr, i, q) → (o, d)` per the kernel-as-pure-fn invariant in
//! `architecture.md` §1), not about its syntactic structure.
//! Whether the kernel uses `match q.state`, multiple matches,
//! nested `if`s, a `dont_care()` + field-set construction, or any
//! other RHDL-legal shape, the transition graph is determined by
//! the *function* the kernel computes — not the AST that defines
//! it.
//!
//! ## Algorithm (per `fsm-architecture.md` §5.3)
//!
//! 1. **Locate the state slot.**  Walk the kernel's return slot
//!    backward through the Tuple → Struct/Splice chain to find the
//!    slot whose value becomes `d.<state_field>` at the kernel's
//!    return point.  In SSA this is unambiguous: every D-component
//!    chain terminates in either a Splice with `path = [<state_field>]`
//!    or a Struct with an explicit `<state_field>` member.  If the
//!    walk fails (return shape isn't recognised), surface a single
//!    kernel-level `Unanalyzable` and stop.
//!
//! 2. **For each source variant `s`:** walk the data-flow graph
//!    backward from the state slot under the constraint
//!    `q.<state_field> = s`.  Constraint propagation is the key:
//!    - At each `Case` opcode whose discriminant is `Index(q,
//!      [<state_field>])`, only the arm whose `CaseArgument` matches
//!      `s`'s discriminant contributes.  Other arms are
//!      constraint-eliminated.  This is what distinguishes the
//!      FSM-transition `Case` from output-computation `match q.<state>`
//!      expressions.
//!    - At each `Splice` with `path = [<state_field>]`, the result
//!      is the substituted value (an explicit transition).
//!    - At each `Splice` with a non-state path, recurse on the
//!      original (the state field is held).
//!    - At each `Index` reading `q.<state_field>`, the result is
//!      `s` itself — this produces the implicit self-loop from the
//!      canonical kernel-top default `d.<state_field> = q.<state_field>`.
//!    - All other ops are walked transparently (`Assign` forwards;
//!      `Select` unions branches; `Case` with non-state discriminant
//!      unions arms).  Opcodes the walker doesn't recognise
//!      contribute empty (no info from this slot).
//!
//! ## What "implicit self-loop" means
//!
//! Production RHDL widgets follow CLAUDE.md §3's "construct via
//! `dont_care()`, then assign every meaningful field" pattern,
//! which for FSM widgets typically means writing `d.<state_field>
//! = q.<state_field>` once at the top of the kernel and only
//! overriding it in arms that actually transition.  An arm with no
//! `d.<state_field>` write — or a conditional inside an arm whose
//! else-branch quietly omits the assignment — is therefore *not* a
//! bug; the data-flow walk falls through to the kernel-top default
//! and reads `q.<state_field>` (which under the constraint
//! `q.<state_field> = s` evaluates to `s`).  The resulting `(s, s)`
//! self-loop is what the principled algorithm computes; no
//! special-case logic is required.
//!
//! ## Diagnostics
//!
//! `Unanalyzable` is reserved for cases where the static analysis
//! cannot derive a sound answer:
//!
//! - **Kernel-level**: the return shape isn't a Tuple containing a
//!   D struct built by recognised ops.
//! - **Arm-level**: a source variant's walk encounters a malformed
//!   IR construct (e.g., an `Enum` template whose discriminant
//!   matches no variant).
//!
//! Each diagnostic carries the source-variant name and a short
//! reason string.  Genuine "no transition info" (an arm that simply
//! holds the state in place via the canonical kernel-top default)
//! is *not* `Unanalyzable` — it's an implicit self-loop, which the
//! principled algorithm computes correctly.
//!
//! ## Acceptance criterion
//!
//! Per `fsm-architecture.md` §5.4: for every FSM-tagged widget in
//! the production corpus, the extractor produces a derived graph
//! without `Unanalyzable` diagnostics, pinned by snapshot tests.
//! The corpus snapshot suite ships with the widget reorganization
//! PR (the corpus widgets do not exist on main yet); the Tier-1
//! adversarial integration tests in `crates/rhdl-fpga/src/doc.rs`
//! cover the same kernel-language idioms via synthetic widgets
//! built directly against the extractor on main.

use crate::fsm::analysis::Transition;
use crate::fsm::descriptor::FsmDescriptor;
use crate::rhif::spec::{CaseArgument, Member, OpCode, Slot};
use crate::types::path::{Path, PathElement};
use crate::types::typed_bits::TypedBits;

/// The output of one extraction call.
///
/// Bundled together so the caller can pass both directly to
/// [`super::analysis::analyze_fsm_structure`].
#[derive(Debug, Clone, Default)]
pub struct ExtractionResult {
    /// Transitions extracted from the kernel.
    pub transitions: Vec<Transition>,
    /// Source variants whose target was unanalysable, with a
    /// short reason.  Reported by the analysis pass as
    /// `FsmDiagnosticKind::Unanalyzable`.
    pub unanalyzable: Vec<(&'static str, &'static str)>,
}

/// Look up the variant index given a discriminant value.
///
/// Returns `None` if no variant in the descriptor matches —
/// indicates the discriminant came from a non-state expression.
fn variant_index_for_discriminant(
    desc: &FsmDescriptor,
    discriminant: i128,
) -> Option<usize> {
    desc.variants()
        .iter()
        .position(|v| v.discriminant == discriminant)
}

/// Pull a `i128` discriminant value out of a `TypedBits`.
///
/// The state enum's discriminant is stored as either an unsigned
/// or signed integer in `TypedBits`.  We normalise to `i128` to
/// match the descriptor's storage convention.
///
/// Decode strategy: read the raw bits, treating `One` as 1 and
/// anything else (including `X`) as 0; this is correct for
/// concrete-discriminant slots, which is the only place we call
/// this from.  If the kind is signed, sign-extend at the
/// width's top bit.
fn typed_bits_to_discriminant(tb: &TypedBits) -> Option<i128> {
    use crate::types::kind::{DiscriminantType, Kind};
    let bools: Vec<bool> = tb
        .iter()
        .map(|b| matches!(b, crate::bitx::BitX::One))
        .collect();
    let mut value: i128 = 0;
    for (i, &b) in bools.iter().enumerate() {
        if b {
            value |= 1_i128 << i;
        }
    }
    let kind = tb.kind();
    match &kind {
        Kind::Signed(width) if *width > 0 => {
            if bools.len() >= *width && bools[*width - 1] {
                let mask = !0_i128 << *width;
                value |= mask;
            }
        }
        Kind::Enum(e) => {
            let width = e.discriminant_layout.width;
            if matches!(e.discriminant_layout.ty, DiscriminantType::Signed)
                && width > 0
                && bools.len() >= width
                && bools[width - 1]
            {
                let mask = !0_i128 << width;
                value |= mask;
            }
        }
        _ => {}
    }
    Some(value)
}

/// Find the opcode (if any) that defines the given slot in `ops`.
///
/// Returns the index into `ops` where the LHS first matches.
fn find_definer<'a>(ops: &'a [OpCode], slot: Slot) -> Option<&'a OpCode> {
    for op in ops.iter().rev() {
        if op.lhs() == Some(slot) {
            return Some(op);
        }
    }
    None
}

/// Extract the source variant index for a `CaseArgument`.
///
/// In RHIF, case arguments are `Slot`s that hold the discriminant
/// of the variant being matched.  The slot is typically a literal
/// `TypedBits` value — we resolve it via the literal table.
///
/// Returns `None` for `CaseArgument::Wild` (the catch-all `_`
/// arm) or when the slot can't be resolved to a known variant.
fn source_variant_for_case_arg(
    desc: &FsmDescriptor,
    arg: &CaseArgument,
    literal_lookup: impl Fn(Slot) -> Option<TypedBits>,
) -> Option<usize> {
    match arg {
        CaseArgument::Wild => None,
        CaseArgument::Slot(slot) => {
            let tb = literal_lookup(*slot)?;
            let disc = typed_bits_to_discriminant(&tb)?;
            variant_index_for_discriminant(desc, disc)
        }
    }
}

/// True if `path` is exactly `[.<state_field>]` — i.e., the path
/// addresses the state field of a struct (typically `D` or `Q`)
/// and nothing nested below it.
fn path_targets_state_field(path: &Path, state_field: &str) -> bool {
    let mut it = path.iter();
    match it.next() {
        Some(PathElement::Field(f)) if f.as_str() == state_field => it.next().is_none(),
        _ => false,
    }
}

/// Step 1 of the algorithm: locate the slot that becomes
/// `d.<state_field>` at the kernel's return point.
///
/// The kernel's return slot is a `Tuple` op for `(o, d)`.  Walking
/// the d-component slot backward through Splice / Struct / Assign
/// ops yields the most recent value spliced into the `<state_field>`
/// path — that is the state slot for the per-variant walks in step 2.
///
/// Returns `Err(reason)` if the return shape isn't recognised
/// (e.g., the kernel returns a non-Tuple slot, or the d-component's
/// chain never sets `<state_field>`).  Per `fsm-architecture.md`
/// §5.3 step 1, this surfaces as a single kernel-level
/// `Unanalyzable` diagnostic with no per-variant walks attempted.
fn find_kernel_return_d_state_slot(
    ops: &[OpCode],
    return_slot: Slot,
    state_field: &str,
) -> Result<Slot, &'static str> {
    // Trace through Assigns to find the actual definer.
    let mut current = return_slot;
    loop {
        match find_definer(ops, current) {
            Some(OpCode::Assign(a)) => current = a.rhs,
            Some(OpCode::Tuple(t)) => {
                // Synchronous kernels return (O, D); the D component
                // is at index 1.
                let d_slot = t
                    .fields
                    .get(1)
                    .copied()
                    .ok_or("kernel return tuple has fewer than 2 elements")?;
                return locate_state_field_slot(ops, d_slot, state_field);
            }
            _ => return Err("kernel return slot is not a (O, D) Tuple"),
        }
    }
}

/// Walk a d-struct slot backward to find the slot whose value is
/// `d.<state_field>`.  Handles the canonical Splice-chain lowering
/// of `let mut d = D::dont_care(); d.<state_field> = ...; ...`.
fn locate_state_field_slot(
    ops: &[OpCode],
    slot: Slot,
    state_field: &str,
) -> Result<Slot, &'static str> {
    let mut current = slot;
    // Walk back through ops, looking for the most recent Splice
    // whose path targets <state_field>, or a Struct with explicit
    // <state_field> member, or an Assign forward.  Loop bound is
    // op count to prevent infinite loops on malformed IR.
    for _ in 0..ops.len().saturating_add(1) {
        let Some(definer) = find_definer(ops, current) else {
            // Current slot has no definer — it's a function arg or
            // literal.  We never found a <state_field> write.
            return Err("kernel d-struct chain never overrides the state field");
        };
        match definer {
            OpCode::Splice(s) if path_targets_state_field(&s.path, state_field) => {
                // Found the most recent override of d.<state_field>.
                // The substituted value IS the state slot.
                return Ok(s.subst);
            }
            OpCode::Splice(s) => {
                // A splice on a non-state field; state held from `orig`.
                current = s.orig;
            }
            OpCode::Assign(a) => {
                current = a.rhs;
            }
            OpCode::Struct(struct_op) => {
                // Explicit struct construction.  If <state_field> is
                // in the explicit field list, that's the state slot;
                // otherwise it comes from the `template` (dont_care)
                // and we have no state slot to walk.
                for fv in &struct_op.fields {
                    if let Member::Named(name) = &fv.member {
                        if name.as_str() == state_field {
                            return Ok(fv.value);
                        }
                    }
                }
                return Err(
                    "kernel d-struct constructed without explicit state field — \
                     state slot is the template value (typically dont_care)",
                );
            }
            OpCode::Select(sel) => {
                // The d-struct itself is conditional (e.g., if-else
                // returning two distinct d's).  We can't reduce this
                // to a single state slot at the locate step, but the
                // per-variant walker handles it via Select union, so
                // we use the slot as-is (the walker dispatches on the
                // Select).
                let _ = sel;
                return Ok(current);
            }
            OpCode::Case(case) => {
                // Same as Select — d-struct returned by a nested
                // match.  Walker handles via Case union.
                let _ = case;
                return Ok(current);
            }
            _ => {
                // Some other op produced the d-struct.  Hand it to
                // the walker as-is and let it bottom out empty if
                // the chain has nothing useful.
                return Ok(current);
            }
        }
    }
    Err("locate_state_field_slot exceeded op-count bound (malformed IR)")
}

/// True if `slot` is reached, transitively through a chain of
/// extraction / boolean ops, from an `Index` reading `.reset` of
/// some struct.  This identifies the canonical RHDL reset block
/// shape: `if cr.reset.any() { d.<state_field> = INIT; ... }`,
/// which lowers to `Select(Unary(OrReduce, Index(cr, [.reset])),
/// d_with_reset_override, d_normal)`.
///
/// The principled FSM transition graph is *non-reset* (per the
/// scoping doc / `fsm-architecture.md` §5 refinement): the manual
/// `FSM_TRANSITIONS` consts in the corpus list only the kernel's
/// non-reset transitions, since reset is conventionally treated as
/// out-of-band rather than as a per-state edge.  When the walker
/// hits a `Select` whose condition is a reset signal, it skips the
/// true-branch and only walks the false-branch.
///
/// Walks back through:
/// - `Assign` (forwards rhs)
/// - `Unary` (e.g., `|r70` = OrReduce, the `.any()` lowering)
/// - `Index` of any kind
/// - Any chain ending in `Index(_, [.reset])`
fn slot_reads_reset_field(ops: &[OpCode], slot: Slot) -> bool {
    let mut current = slot;
    for _ in 0..ops.len().saturating_add(1) {
        let Some(definer) = find_definer(ops, current) else {
            return false;
        };
        match definer {
            OpCode::Index(idx) => {
                // Check if this Index targets the .reset field.
                let mut it = idx.path.iter();
                if let Some(PathElement::Field(f)) = it.next() {
                    if f.as_str() == "reset" && it.next().is_none() {
                        return true;
                    }
                }
                // Otherwise, walk back through the indexed slot.
                current = idx.arg;
            }
            OpCode::Assign(a) => current = a.rhs,
            OpCode::Unary(u) => current = u.arg1,
            _ => return false,
        }
    }
    false
}

/// Try to statically resolve a `Select` condition slot under the
/// constraint `q.<state_field> = source_variant`.  Returns:
/// - `Some(true)` if the condition definitely holds under the constraint.
/// - `Some(false)` if the condition definitely doesn't hold.
/// - `None` if the condition isn't of a recognised statically-resolvable
///   shape, or the literal operand doesn't decode to a known variant.
///
/// Recognised shape: `Binary(Eq, lhs, rhs)` where one of `lhs`/`rhs`
/// traces back to an `Index` reading `q.<state_field>` (possibly
/// through an EnumDiscriminant extraction Index) and the other is a
/// state-typed literal whose discriminant decodes via the descriptor.
///
/// This implements `fsm-architecture.md` §5.3 step 2a's constraint
/// propagation through Select on state-equality comparisons.  It
/// tightens the over-approximation budget for kernels with
/// `if q.<state_field> == StateX { ... }` inside transition logic
/// (per §5.4.2 #1).  Sound by construction: returning `None` is
/// always safe (the caller falls back to union).
fn resolve_state_eq_condition(
    desc: &FsmDescriptor,
    ops: &[OpCode],
    cond: Slot,
    state_field: &str,
    source_variant: usize,
    literal_lookup: &impl Fn(Slot) -> Option<TypedBits>,
) -> Option<bool> {
    let definer = find_definer(ops, cond)?;
    let bin = match definer {
        OpCode::Binary(b) if matches!(b.op, crate::rhif::spec::AluBinary::Eq) => b,
        _ => return None,
    };

    // Identify which arg reads q.<state_field> and which is the literal.
    let arg1_is_state = slot_reads_state_field(ops, bin.arg1, state_field);
    let arg2_is_state = slot_reads_state_field(ops, bin.arg2, state_field);
    let lit_arg = match (arg1_is_state, arg2_is_state) {
        (true, false) => bin.arg2,
        (false, true) => bin.arg1,
        _ => return None, // both or neither — not a state-eq comparison
    };

    // Resolve the literal arg's discriminant.  May be a literal slot
    // OR an Enum opcode constructing the discriminant.
    let lit_disc = if let Some(tb) = literal_lookup(lit_arg) {
        typed_bits_to_discriminant(&tb)?
    } else {
        match find_definer(ops, lit_arg)? {
            OpCode::Enum(e) => typed_bits_to_discriminant(&e.template)?,
            _ => return None,
        }
    };

    let lit_variant = variant_index_for_discriminant(desc, lit_disc)?;
    Some(lit_variant == source_variant)
}

/// True if `slot` is reached, transitively through a chain of
/// extraction ops, from `Index(_, [<state_field>])` — i.e., the
/// slot's value is computed from the state field of some struct
/// (typically `q`).  Used by the constraint-propagation logic to
/// recognise `q.<state_field>` reads in Case discriminants and
/// Select conditions.
///
/// Walks back through:
/// - `Assign` (forwards rhs)
/// - `Index(_, [#])` — the discriminant-extracting Index that the
///   compiler inserts in front of every `match q.state` Case to
///   feed its discriminant input.  The chain `Case ← Index([#]) ←
///   Index([.state]) ← q` is the canonical lowering.
/// - `Index` with any other path: also traversed (the extraction
///   chain may include further nested field accesses).
///
/// Returns `true` when the chain bottoms out at an `Index` whose
/// path targets `<state_field>`; `false` if it bottoms out at any
/// other op (a binary op, a function arg with no path, etc.).
fn slot_reads_state_field(ops: &[OpCode], slot: Slot, state_field: &str) -> bool {
    let mut current = slot;
    for _ in 0..ops.len().saturating_add(1) {
        let Some(definer) = find_definer(ops, current) else {
            return false;
        };
        match definer {
            OpCode::Index(idx) if path_targets_state_field(&idx.path, state_field) => {
                return true;
            }
            OpCode::Index(idx) => {
                // Any other Index (e.g., EnumDiscriminant extraction
                // `r#` that the compiler inserts in front of every
                // Case on an enum) — walk back through its `arg`.
                current = idx.arg;
            }
            OpCode::Assign(a) => current = a.rhs,
            _ => return false,
        }
    }
    false
}

/// Step 2 of the algorithm: walk the data-flow graph backward from
/// `slot` and collect all possible values of `d.<state_field>`
/// under the constraint `q.<state_field> == source_variant`.
///
/// Constraint propagation is the key.  Without it, a kernel with
/// multiple `match q.<state>` expressions (output computation +
/// transition logic) would produce an over-approximation that
/// includes targets from the wrong matches.  With it:
///
/// - At a `Case` opcode whose discriminant reads `q.<state_field>`,
///   only the arm matching the source variant's discriminant
///   contributes (a `Wild` arm catches if no specific arm matches).
///   Other arms are constraint-eliminated.
/// - At a `Select`, both branches are unioned (we don't yet
///   resolve `q.<state_field> == X` comparisons in Select
///   conditions; conservative over-approximation is sound).
/// - At an `Index` reading `q.<state_field>`, the result is the
///   source variant index — this is the implicit self-loop from
///   the canonical kernel-top default `d.<state_field> = q.<state_field>`.
///
/// Returns `Ok(set of variant indices)` on success, or `Err(reason)`
/// for an arm-level Unanalyzable.  An empty `Ok` set means "no
/// state-typed value flows from this slot under the constraint" —
/// the caller treats this as a contributing self-loop at union
/// points (Select / Case branches) and as a single self-loop edge
/// at the top level.
fn possible_state_values_under_constraint(
    desc: &FsmDescriptor,
    ops: &[OpCode],
    slot: Slot,
    state_field: &str,
    source_variant: usize,
    literal_lookup: &impl Fn(Slot) -> Option<TypedBits>,
    allow_implicit: bool,
) -> Result<std::collections::BTreeSet<usize>, &'static str> {
    use std::collections::BTreeSet;

    // Literal of the state type — direct discriminant lookup.
    if let Some(tb) = literal_lookup(slot) {
        if let Some(disc) = typed_bits_to_discriminant(&tb) {
            if let Some(idx) = variant_index_for_discriminant(desc, disc) {
                return Ok(BTreeSet::from([idx]));
            }
        }
    }

    let Some(definer) = find_definer(ops, slot) else {
        // Slot is a function argument or unresolved literal; no
        // state value flows from here.
        return Ok(BTreeSet::new());
    };

    match definer {
        // --- terminal: explicit state value construction ---

        OpCode::Enum(e) => {
            let disc = typed_bits_to_discriminant(&e.template)
                .ok_or("enum template has no resolvable discriminant")?;
            let idx = variant_index_for_discriminant(desc, disc)
                .ok_or("enum discriminant doesn't match any variant")?;
            Ok(BTreeSet::from([idx]))
        }

        // --- terminal: implicit self-loop from kernel-top default ---

        // Index reading <some>.<state_field>: this is the
        // `q.<state_field>` read that the canonical kernel-top
        // default `d.<state_field> = q.<state_field>` produces.
        // Under the constraint q.<state_field> == s, it evaluates
        // to s — emit the self-loop iff the widget opts in via
        // `#[fsm(allow_implicit)]`.  When the widget didn't opt
        // in, this path contributes nothing, so a state whose
        // only would-be outgoing edge was the implicit self-loop
        // ends up with no outgoing edges → DeadlockCandidate
        // fires (per `fsm-architecture.md` §5.4.1, closing the
        // gap by structural opt-in).
        OpCode::Index(idx) if path_targets_state_field(&idx.path, state_field) => {
            if allow_implicit {
                Ok(BTreeSet::from([source_variant]))
            } else {
                Ok(BTreeSet::new())
            }
        }

        // --- forwarding cases ---

        OpCode::Assign(a) => possible_state_values_under_constraint(
            desc,
            ops,
            a.rhs,
            state_field,
            source_variant,
            literal_lookup,
            allow_implicit,
        ),

        // --- Splice: state-field-aware ---

        OpCode::Splice(s) => {
            if path_targets_state_field(&s.path, state_field) {
                // Explicit override of d.<state_field> by `subst`.
                possible_state_values_under_constraint(
                    desc,
                    ops,
                    s.subst,
                    state_field,
                    source_variant,
                    literal_lookup,
                    allow_implicit,
                )
            } else {
                // Splice into a different field; state held from `orig`.
                possible_state_values_under_constraint(
                    desc,
                    ops,
                    s.orig,
                    state_field,
                    source_variant,
                    literal_lookup,
                    allow_implicit,
                )
            }
        }

        // --- Struct: explicit field set or template fall-through ---

        OpCode::Struct(struct_op) => {
            for fv in &struct_op.fields {
                if let Member::Named(name) = &fv.member {
                    if name.as_str() == state_field {
                        return possible_state_values_under_constraint(
                            desc,
                            ops,
                            fv.value,
                            state_field,
                            source_variant,
                            literal_lookup,
                            allow_implicit,
                        );
                    }
                }
            }
            // Field comes from template — no state value here.
            Ok(BTreeSet::new())
        }

        // --- Select: conditional ---
        // ---   (1) reset-condition special case → skip true     ---
        // ---   (2) q.<state_field> == X comparison              ---
        // ---       statically resolvable under constraint       ---
        // ---   (3) otherwise: union both branches               ---

        OpCode::Select(sel) => {
            // (1) Reset special case: the canonical RHDL kernel pattern
            // `if cr.reset.any() { d.<state_field> = INIT; ... }`
            // lowers to a Select where the condition reads
            // cr.reset.  Per the FSM transition graph convention
            // (manual FSM_TRANSITIONS lists are non-reset), skip
            // the true-branch and only walk the false-branch.
            if slot_reads_reset_field(ops, sel.cond) {
                return possible_state_values_under_constraint(
                    desc,
                    ops,
                    sel.false_value,
                    state_field,
                    source_variant,
                    literal_lookup,
                    allow_implicit,
                );
            }

            // (2) q.<state_field> == X comparison: try to statically
            // resolve under the constraint q.<state_field> = source_variant.
            // If the comparison's discriminant matches source_variant,
            // only the true-branch contributes; if it explicitly doesn't,
            // only the false-branch.  This tightens the over-approximation
            // budget for kernels with `if q.<state_field> == StateX { ... }`
            // inside transition logic (per fsm-architecture.md §5.4.2 #1).
            if let Some(resolved) = resolve_state_eq_condition(
                desc,
                ops,
                sel.cond,
                state_field,
                source_variant,
                literal_lookup,
            ) {
                let chosen = if resolved { sel.true_value } else { sel.false_value };
                return possible_state_values_under_constraint(
                    desc,
                    ops,
                    chosen,
                    state_field,
                    source_variant,
                    literal_lookup,
                    allow_implicit,
                );
            }

            // (3) Default: union both branches.
            let mut t = possible_state_values_under_constraint(
                desc,
                ops,
                sel.true_value,
                state_field,
                source_variant,
                literal_lookup,
                allow_implicit,
            )?;
            let mut f = possible_state_values_under_constraint(
                desc,
                ops,
                sel.false_value,
                state_field,
                source_variant,
                literal_lookup,
                allow_implicit,
            )?;
            // Empty branch contributes a self-loop at the union iff
            // the widget opts in via `#[fsm(allow_implicit)]`.  When
            // it doesn't, the branch's "no state write" stays "no
            // contribution" — the analysis layer will see the
            // missing edge and fire DeadlockCandidate if appropriate.
            if allow_implicit {
                if t.is_empty() {
                    t.insert(source_variant);
                }
                if f.is_empty() {
                    f.insert(source_variant);
                }
            }
            t.extend(f);
            Ok(t)
        }

        // --- Case: nested match, with constraint propagation ---

        OpCode::Case(case) => {
            // Critical step: if this Case's discriminant reads
            // q.<state_field>, only the arm matching the source
            // variant contributes.  Other arms are
            // constraint-eliminated.  This is what makes the
            // extractor work on multi-match kernels (output
            // computation matches don't pollute the transition
            // graph).
            if slot_reads_state_field(ops, case.discriminant, state_field) {
                let source_disc = desc.variants()[source_variant].discriminant;
                let mut matched_arm: Option<Slot> = None;
                let mut wild_arm: Option<Slot> = None;
                for (arg, arm_slot) in &case.table {
                    match arg {
                        CaseArgument::Wild => {
                            wild_arm = Some(*arm_slot);
                        }
                        CaseArgument::Slot(disc_slot) => {
                            if let Some(tb) = literal_lookup(*disc_slot) {
                                if let Some(disc) = typed_bits_to_discriminant(&tb) {
                                    if disc == source_disc {
                                        matched_arm = Some(*arm_slot);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                let arm_slot = matched_arm
                    .or(wild_arm)
                    .ok_or("Case on q.<state_field> has no arm matching this variant and no wild arm")?;
                let mut result = possible_state_values_under_constraint(
                    desc,
                    ops,
                    arm_slot,
                    state_field,
                    source_variant,
                    literal_lookup,
                    allow_implicit,
                )?;
                // Empty arm contributes a self-loop iff the widget
                // opts in via `#[fsm(allow_implicit)]`.
                if allow_implicit && result.is_empty() {
                    result.insert(source_variant);
                }
                Ok(result)
            } else {
                // Case on a non-state discriminant (e.g., match on
                // q.dlc_reg, or i.start, or a computed bool).  Union
                // all arms — the result depends on input we can't
                // constrain.
                let mut union = BTreeSet::new();
                for (_arg, arm_slot) in &case.table {
                    let mut arm = possible_state_values_under_constraint(
                        desc,
                        ops,
                        *arm_slot,
                        state_field,
                        source_variant,
                        literal_lookup,
                        allow_implicit,
                    )?;
                    if allow_implicit && arm.is_empty() {
                        arm.insert(source_variant);
                    }
                    union.extend(arm);
                }
                Ok(union)
            }
        }

        // --- Anything else: no state value flows from here ---

        _ => Ok(BTreeSet::new()),
    }
}

/// Lookup helper: produce a closure that resolves `Slot::Literal`
/// to its `TypedBits` from a literal table provided by the caller.
///
/// The caller is expected to construct this table from their
/// `Object`'s `symtab` (which maps literal IDs to TypedBits).
/// We accept a closure rather than the full symtab so the
/// extractor doesn't have to depend on the symtab's internals.
pub type LiteralLookup<'a> = &'a dyn Fn(Slot) -> Option<TypedBits>;

/// Extract the FSM transition graph from a kernel's RHIF.
///
/// `ops` is the kernel's full op list.  `return_slot` is the slot
/// that holds the kernel's return value (per the RHIF `Object`'s
/// `return_slot` field).  `desc` is the FSM descriptor for this
/// widget.  `literal_lookup` resolves `Slot::Literal` slots to
/// their underlying `TypedBits` (the caller wires this up from the
/// kernel `Object`'s symbol table).
///
/// Implements the principled algorithm per the module-level
/// docstring and `fsm-architecture.md` §5.3:
///
/// 1. Locate the slot that becomes `d.<state_field>` at the
///    kernel's return point.
/// 2. For each source variant `s`, walk that slot's data flow
///    backward under constraint `q.<state_field> = s` and collect
///    all possible target values.
/// 3. Emit transitions for each `(s, t)` found; emit per-variant
///    `Unanalyzable` diagnostics for arms whose walk fails.
///
/// The output is `ExtractionResult { transitions, unanalyzable }`
/// with transitions deduplicated and a per-variant diagnostic for
/// any source variant whose walk produced an error.
pub fn extract_canonical_transitions(
    ops: &[OpCode],
    return_slot: Slot,
    desc: &FsmDescriptor,
    literal_lookup: LiteralLookup<'_>,
) -> ExtractionResult {
    use std::collections::BTreeSet;
    let mut result = ExtractionResult::default();

    let state_field = desc.widget.state_field;

    // Step 1: locate the state slot at the kernel's return point.
    let state_slot = match find_kernel_return_d_state_slot(ops, return_slot, state_field) {
        Ok(s) => s,
        Err(reason) => {
            // Kernel-level Unanalyzable: the return shape isn't
            // recognised, so we can't even start the per-variant
            // walks.  Surface a single diagnostic against a
            // synthetic source-name "<kernel>" so the analysis
            // pass can flag the whole kernel.
            result.unanalyzable.push(("<kernel>", reason));
            return result;
        }
    };

    // Step 2: per-variant constrained walk.
    let allow_implicit = desc.widget.allow_implicit;
    let mut seen: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (source_idx, source_desc) in desc.variants().iter().enumerate() {
        let walk_result = possible_state_values_under_constraint(
            desc,
            ops,
            state_slot,
            state_field,
            source_idx,
            &literal_lookup,
            allow_implicit,
        );
        match walk_result {
            Ok(targets) => {
                let targets = if targets.is_empty() && allow_implicit {
                    // The state slot's chain made no mention of
                    // q.<state_field> AND no explicit override
                    // along any path.  Per the implicit-self-loop
                    // convention (opt-in via #[fsm(allow_implicit)]),
                    // this is a self-loop.  Without the opt-in,
                    // the empty result stays empty — the analysis
                    // layer will see no outgoing edges and fire
                    // DeadlockCandidate.
                    BTreeSet::from([source_idx])
                } else {
                    targets
                };
                for target_idx in targets {
                    if seen.insert((source_idx, target_idx)) {
                        result.transitions.push(Transition {
                            source_index: source_idx,
                            target_index: target_idx,
                        });
                    }
                }
            }
            Err(reason) => {
                result.unanalyzable.push((source_desc.name, reason));
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsm::descriptor::{FsmKernelTag, FsmWidgetTag};
    use crate::fsm::state::FsmVariantDescriptor;
    use crate::common::slot_vec::SlotKey;
    use crate::rhif::rhif_builder::{op_assign, op_case, op_enum, op_index, op_select, op_splice, op_tuple};
    use crate::rhif::spec::Slot;
    use crate::types::typed_bits::TypedBits;
    use std::collections::BTreeMap;

    static THREE: &[FsmVariantDescriptor] = &[
        FsmVariantDescriptor {
            name: "Idle",
            discriminant: 0,
            has_payload: false,
            terminal: false,
            label: None,
        },
        FsmVariantDescriptor {
            name: "Running",
            discriminant: 1,
            has_payload: false,
            terminal: false,
            label: None,
        },
        FsmVariantDescriptor {
            name: "Done",
            discriminant: 2,
            has_payload: false,
            terminal: false,
            label: None,
        },
    ];

    fn three_state_descriptor() -> FsmDescriptor {
        // Default: allow_implicit = true.  The vast majority of
        // pre-existing tests pin the canonical RHDL kernel pattern
        // (kernel-top default + implicit self-loops on un-overridden
        // arms), which corresponds to widgets that have opted in
        // via #[fsm(allow_implicit)].  Tests that exercise the
        // strict mode use `three_state_strict_descriptor()`.
        FsmDescriptor {
            widget_name: "test::Three",
            widget: FsmWidgetTag {
                state_field: "state",
                strict: false,
                allow_implicit: true,
            },
            kernel: FsmKernelTag {
                state_var: "q.state",
            },
            variants_fn: || THREE,
            initial_fn: || 0,
        }
    }

    /// Strict-mode descriptor: `allow_implicit = false`.  Used by
    /// tests that exercise the strict deadlock-detection mode where
    /// only explicitly-written transitions count.
    fn three_state_strict_descriptor() -> FsmDescriptor {
        FsmDescriptor {
            widget_name: "test::ThreeStrict",
            widget: FsmWidgetTag {
                state_field: "state",
                strict: false,
                allow_implicit: false,
            },
            kernel: FsmKernelTag {
                state_var: "q.state",
            },
            variants_fn: || THREE,
            initial_fn: || 0,
        }
    }

    /// Simulate the literal table — just a wrapper around a
    /// `BTreeMap<Slot, TypedBits>` that satisfies the
    /// `LiteralLookup` signature.
    fn make_lookup(
        table: BTreeMap<Slot, TypedBits>,
    ) -> impl Fn(Slot) -> Option<TypedBits> {
        move |s: Slot| table.get(&s).cloned()
    }

    fn lit_disc_unsigned(value: u128, width: usize) -> TypedBits {
        // Build a TypedBits manually: kind = Bits(width), bits = LSB-first.
        let mut bits = Vec::with_capacity(width);
        for i in 0..width {
            let bit = (value >> i) & 1 == 1;
            bits.push(if bit {
                crate::bitx::BitX::One
            } else {
                crate::bitx::BitX::Zero
            });
        }
        TypedBits::new(bits, crate::types::kind::Kind::Bits(width))
    }

    fn make_slot(reg: usize) -> Slot {
        Slot::Literal(crate::common::symtab::LiteralId::new(reg as u64, reg))
    }

    fn make_register(reg: usize) -> Slot {
        Slot::Register(crate::common::symtab::RegisterId::new(reg as u64, reg))
    }

    /// Path `.state` — the standard state-field path used in test descriptors.
    fn state_path() -> Path {
        Path::default().field("state")
    }

    /// Build a literal `dont_care` D struct for use as the kernel's
    /// initial d value (typically the start of a Splice chain).
    fn lit_d_dont_care() -> TypedBits {
        // Empty bytes; kind is irrelevant for the walker.
        TypedBits::new(vec![], crate::types::kind::Kind::Empty)
    }

    /// Helper: assemble a synthetic kernel return.  Wraps the
    /// caller's d-struct slot in a Tuple `(o, d)` so the principled
    /// algorithm's locate-step can find the d-component.
    ///
    /// Returns the appended ops and the slot to pass as `return_slot`.
    fn wrap_return(
        ops: &mut Vec<OpCode>,
        next_reg: usize,
        d_slot: Slot,
        o_slot: Slot,
    ) -> Slot {
        let return_slot = make_register(next_reg);
        ops.push(op_tuple(return_slot, vec![o_slot, d_slot]));
        return_slot
    }

    // =====================================================
    // Tests for the principled algorithm.
    //
    // Each test constructs a synthetic kernel matching a specific
    // RHIF shape (output computation match, transition match,
    // implicit self-loop via kernel-top default, etc.) and verifies
    // the extractor produces the right transition graph under
    // constraint propagation.
    // =====================================================

    /// Most basic test: kernel with a single FSM-transition Case
    /// whose discriminant reads q.state.  Each arm assigns
    /// d.state = <constant> (side-effect form via Splice).
    /// Verifies the principled algorithm recovers the canonical
    /// 3-state cycle.
    #[test]
    fn principled_extracts_canonical_three_state_cycle() {
        let q_reg = make_register(100); // function arg q
        let o_dummy = make_register(101); // dummy output

        let r_d_init = make_register(0); // d = dont_care
        let r_q_state = make_register(1); // q.state
        let r_d_default = make_register(2); // d.state = q.state (kernel-top default)
        let r_to_running = make_register(3); // d.state = Running
        let r_to_done = make_register(4); // d.state = Done
        let r_to_idle = make_register(5); // d.state = Idle
        let r_arm_idle = make_register(6); // d after Idle arm
        let r_arm_running = make_register(7); // d after Running arm
        let r_arm_done = make_register(8); // d after Done arm
        let r_running_enum = make_register(10); // Enum(Running)
        let r_done_enum = make_register(11); // Enum(Done)
        let r_idle_enum = make_register(12); // Enum(Idle)
        let r_d_after_case = make_register(13); // d after the FSM Case
        let sl_dont_care = make_slot(50);
        let lit_idle = make_slot(51); // CaseArgument for Idle (disc=0)
        let lit_running = make_slot(52); // CaseArgument for Running (disc=1)
        let lit_done = make_slot(53); // CaseArgument for Done (disc=2)

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());
        lookup.insert(lit_idle, lit_disc_unsigned(0, 2));
        lookup.insert(lit_running, lit_disc_unsigned(1, 2));
        lookup.insert(lit_done, lit_disc_unsigned(2, 2));

        let mut ops = vec![
            op_assign(r_d_init, sl_dont_care),
            op_index(r_q_state, q_reg, state_path()),
            op_splice(r_d_default, r_d_init, state_path(), r_q_state),
            op_enum(r_running_enum, vec![], lit_disc_unsigned(1, 2)),
            op_enum(r_done_enum, vec![], lit_disc_unsigned(2, 2)),
            op_enum(r_idle_enum, vec![], lit_disc_unsigned(0, 2)),
            // d in each arm: splice on top of the kernel-top default.
            op_splice(r_arm_idle, r_d_default, state_path(), r_running_enum),
            op_splice(r_arm_running, r_d_default, state_path(), r_done_enum),
            op_splice(r_arm_done, r_d_default, state_path(), r_idle_enum),
            // FSM Case: discriminant is q.state, arms are the per-arm d's.
            op_case(
                r_d_after_case,
                r_q_state,
                vec![
                    (CaseArgument::Slot(lit_idle), r_arm_idle),
                    (CaseArgument::Slot(lit_running), r_arm_running),
                    (CaseArgument::Slot(lit_done), r_arm_done),
                ],
            ),
        ];
        let return_slot = wrap_return(&mut ops, 14, r_d_after_case, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert!(
            result.unanalyzable.is_empty(),
            "got: {:?}",
            result.unanalyzable
        );
        let mut t = result.transitions.clone();
        t.sort();
        assert_eq!(
            t,
            vec![
                Transition { source_index: 0, target_index: 1 }, // Idle → Running
                Transition { source_index: 1, target_index: 2 }, // Running → Done
                Transition { source_index: 2, target_index: 0 }, // Done → Idle
            ]
        );
    }

    /// THE MOTIVATING TEST.  Multi-match kernel: an output-computation
    /// Case appears BEFORE the transition Case in the op list.  Per
    /// the principled algorithm, the constraint propagation
    /// distinguishes the two — only the transition Case (whose result
    /// flows into d.state) contributes to the graph.  The output
    /// Case is irrelevant.
    ///
    /// Pre-fix, the heuristic extractor would have read the FIRST
    /// Case (output computation, returning bool) and produced
    /// nonsense edges.  This test pins that the principled algorithm
    /// ignores the output Case entirely (it's not on the d.state
    /// data path).
    #[test]
    fn principled_ignores_output_computation_match_on_q_state() {
        let q_reg = make_register(100);
        let o_dummy = make_register(101);

        // --- the output-computation match (returns bool, not state) ---
        let r_q_state_a = make_register(20);
        let r_false_lit = make_register(21);
        let r_true_lit = make_register(22);
        let r_output_bool = make_register(23); // result of the output Case
        let lit_idle_a = make_slot(60);
        let lit_running_a = make_slot(61);
        let lit_done_a = make_slot(62);

        // --- the transition match (returns d-struct) ---
        let r_d_init = make_register(0);
        let r_q_state_b = make_register(1);
        let r_d_default = make_register(2);
        let r_to_running = make_register(3);
        let r_to_done = make_register(4);
        let r_to_idle = make_register(5);
        let r_arm_idle = make_register(6);
        let r_arm_running = make_register(7);
        let r_arm_done = make_register(8);
        let r_d_after_case = make_register(9);
        let sl_dont_care = make_slot(50);
        let sl_false = make_slot(63);
        let sl_true = make_slot(64);
        let lit_idle_b = make_slot(51);
        let lit_running_b = make_slot(52);
        let lit_done_b = make_slot(53);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());
        lookup.insert(lit_idle_a, lit_disc_unsigned(0, 2));
        lookup.insert(lit_running_a, lit_disc_unsigned(1, 2));
        lookup.insert(lit_done_a, lit_disc_unsigned(2, 2));
        lookup.insert(lit_idle_b, lit_disc_unsigned(0, 2));
        lookup.insert(lit_running_b, lit_disc_unsigned(1, 2));
        lookup.insert(lit_done_b, lit_disc_unsigned(2, 2));
        lookup.insert(sl_false, lit_disc_unsigned(0, 1));
        lookup.insert(sl_true, lit_disc_unsigned(1, 1));

        let mut ops = vec![
            // Output computation: let bool_out = match q.state { Idle => false, ... }
            op_index(r_q_state_a, q_reg, state_path()),
            op_assign(r_false_lit, sl_false),
            op_assign(r_true_lit, sl_true),
            op_case(
                r_output_bool,
                r_q_state_a,
                vec![
                    (CaseArgument::Slot(lit_idle_a), r_false_lit),
                    (CaseArgument::Slot(lit_running_a), r_true_lit),
                    (CaseArgument::Slot(lit_done_a), r_false_lit),
                ],
            ),
            // Transition logic: same shape as the canonical test above.
            op_assign(r_d_init, sl_dont_care),
            op_index(r_q_state_b, q_reg, state_path()),
            op_splice(r_d_default, r_d_init, state_path(), r_q_state_b),
            op_enum(r_to_running, vec![], lit_disc_unsigned(1, 2)),
            op_enum(r_to_done, vec![], lit_disc_unsigned(2, 2)),
            op_enum(r_to_idle, vec![], lit_disc_unsigned(0, 2)),
            op_splice(r_arm_idle, r_d_default, state_path(), r_to_running),
            op_splice(r_arm_running, r_d_default, state_path(), r_to_done),
            op_splice(r_arm_done, r_d_default, state_path(), r_to_idle),
            op_case(
                r_d_after_case,
                r_q_state_b,
                vec![
                    (CaseArgument::Slot(lit_idle_b), r_arm_idle),
                    (CaseArgument::Slot(lit_running_b), r_arm_running),
                    (CaseArgument::Slot(lit_done_b), r_arm_done),
                ],
            ),
        ];
        let return_slot = wrap_return(&mut ops, 30, r_d_after_case, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert!(
            result.unanalyzable.is_empty(),
            "got: {:?}",
            result.unanalyzable
        );
        let mut t = result.transitions.clone();
        t.sort();
        assert_eq!(
            t,
            vec![
                Transition { source_index: 0, target_index: 1 }, // Idle → Running
                Transition { source_index: 1, target_index: 2 }, // Running → Done
                Transition { source_index: 2, target_index: 0 }, // Done → Idle
            ],
            "output-computation Case must NOT contribute to the transition graph"
        );
    }

    /// Implicit self-loop from the canonical kernel-top default.
    /// Kernel writes `d.state = q.state` at the top, then has no
    /// further d.state writes.  Every variant should produce a
    /// self-loop (held state).
    #[test]
    fn principled_kernel_top_default_alone_yields_all_self_loops() {
        let q_reg = make_register(100);
        let o_dummy = make_register(101);

        let r_d_init = make_register(0);
        let r_q_state = make_register(1);
        let r_d_default = make_register(2);
        let sl_dont_care = make_slot(50);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());

        let mut ops = vec![
            op_assign(r_d_init, sl_dont_care),
            op_index(r_q_state, q_reg, state_path()),
            op_splice(r_d_default, r_d_init, state_path(), r_q_state),
        ];
        let return_slot = wrap_return(&mut ops, 3, r_d_default, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert!(result.unanalyzable.is_empty());
        let mut t = result.transitions.clone();
        t.sort();
        assert_eq!(
            t,
            vec![
                Transition { source_index: 0, target_index: 0 },
                Transition { source_index: 1, target_index: 1 },
                Transition { source_index: 2, target_index: 2 },
            ]
        );
    }

    /// Guarded transition with implicit-hold else-branch (the
    /// can_master Id-arm shape).  Kernel: `d.state = q.state;
    /// match q.state { Idle => if cond { d.state = Running } else
    /// { /* no d.state write */ } ... }`.  Verifies the principled
    /// algorithm produces (Idle → Running) AND (Idle → Idle) from
    /// the one arm.
    #[test]
    fn principled_guarded_transition_emits_explicit_plus_self_loop() {
        let q_reg = make_register(100);
        let o_dummy = make_register(101);
        let cond_reg = make_register(102);

        let r_d_init = make_register(0);
        let r_q_state = make_register(1);
        let r_d_default = make_register(2);
        let r_to_running = make_register(3);
        let r_d_taken = make_register(4); // d after taken-branch splice
        let r_arm_idle = make_register(5); // Select(cond, taken, default)
        let r_d_after_case = make_register(6);
        let sl_dont_care = make_slot(50);
        let lit_idle = make_slot(51);
        let lit_running = make_slot(52);
        let lit_done = make_slot(53);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());
        lookup.insert(lit_idle, lit_disc_unsigned(0, 2));
        lookup.insert(lit_running, lit_disc_unsigned(1, 2));
        lookup.insert(lit_done, lit_disc_unsigned(2, 2));

        let mut ops = vec![
            op_assign(r_d_init, sl_dont_care),
            op_index(r_q_state, q_reg, state_path()),
            op_splice(r_d_default, r_d_init, state_path(), r_q_state),
            op_enum(r_to_running, vec![], lit_disc_unsigned(1, 2)),
            // taken: d.state = Running
            op_splice(r_d_taken, r_d_default, state_path(), r_to_running),
            // arm body: if cond { taken } else { default (state held) }
            op_select(r_arm_idle, cond_reg, r_d_taken, r_d_default),
            op_case(
                r_d_after_case,
                r_q_state,
                vec![
                    (CaseArgument::Slot(lit_idle), r_arm_idle),
                    (CaseArgument::Slot(lit_running), r_d_default),
                    (CaseArgument::Slot(lit_done), r_d_default),
                ],
            ),
        ];
        let return_slot = wrap_return(&mut ops, 7, r_d_after_case, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert!(
            result.unanalyzable.is_empty(),
            "got: {:?}",
            result.unanalyzable
        );
        let mut t = result.transitions.clone();
        t.sort();
        assert_eq!(
            t,
            vec![
                Transition { source_index: 0, target_index: 0 }, // Idle → Idle (else)
                Transition { source_index: 0, target_index: 1 }, // Idle → Running (then)
                Transition { source_index: 1, target_index: 1 }, // Running → Running (held)
                Transition { source_index: 2, target_index: 2 }, // Done → Done (held)
            ]
        );
    }

    /// Or-pattern arm: a single Case arm matches multiple variants.
    /// The principled algorithm's constraint propagation should
    /// produce the same transition for every source variant in the
    /// or-pattern.  Modelled by giving the same `arm_slot` to two
    /// CaseArguments with different discriminants.
    #[test]
    fn principled_or_pattern_arm_distributes_per_source() {
        let q_reg = make_register(100);
        let o_dummy = make_register(101);

        let r_d_init = make_register(0);
        let r_q_state = make_register(1);
        let r_d_default = make_register(2);
        let r_to_done = make_register(3);
        let r_arm_combined = make_register(4); // d.state = Done
        let r_d_after_case = make_register(5);
        let sl_dont_care = make_slot(50);
        let lit_idle = make_slot(51);
        let lit_running = make_slot(52);
        let lit_done = make_slot(53);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());
        lookup.insert(lit_idle, lit_disc_unsigned(0, 2));
        lookup.insert(lit_running, lit_disc_unsigned(1, 2));
        lookup.insert(lit_done, lit_disc_unsigned(2, 2));

        let mut ops = vec![
            op_assign(r_d_init, sl_dont_care),
            op_index(r_q_state, q_reg, state_path()),
            op_splice(r_d_default, r_d_init, state_path(), r_q_state),
            op_enum(r_to_done, vec![], lit_disc_unsigned(2, 2)),
            op_splice(r_arm_combined, r_d_default, state_path(), r_to_done),
            // Or-pattern: Idle | Running both go to Done; Done holds.
            op_case(
                r_d_after_case,
                r_q_state,
                vec![
                    (CaseArgument::Slot(lit_idle), r_arm_combined),
                    (CaseArgument::Slot(lit_running), r_arm_combined),
                    (CaseArgument::Slot(lit_done), r_d_default),
                ],
            ),
        ];
        let return_slot = wrap_return(&mut ops, 6, r_d_after_case, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert!(result.unanalyzable.is_empty());
        let mut t = result.transitions.clone();
        t.sort();
        assert_eq!(
            t,
            vec![
                Transition { source_index: 0, target_index: 2 }, // Idle → Done
                Transition { source_index: 1, target_index: 2 }, // Running → Done
                Transition { source_index: 2, target_index: 2 }, // Done → Done (held)
            ]
        );
    }

    /// Wild arm: the catch-all `_ =>` branch should apply to any
    /// source variant not explicitly listed.
    #[test]
    fn principled_wild_arm_catches_unmatched_variants() {
        let q_reg = make_register(100);
        let o_dummy = make_register(101);

        let r_d_init = make_register(0);
        let r_q_state = make_register(1);
        let r_d_default = make_register(2);
        let r_to_idle = make_register(3);
        let r_arm_to_idle = make_register(4); // wild arm body: d.state = Idle
        let r_d_after_case = make_register(5);
        let sl_dont_care = make_slot(50);
        let lit_idle = make_slot(51);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());
        lookup.insert(lit_idle, lit_disc_unsigned(0, 2));

        let mut ops = vec![
            op_assign(r_d_init, sl_dont_care),
            op_index(r_q_state, q_reg, state_path()),
            op_splice(r_d_default, r_d_init, state_path(), r_q_state),
            op_enum(r_to_idle, vec![], lit_disc_unsigned(0, 2)),
            op_splice(r_arm_to_idle, r_d_default, state_path(), r_to_idle),
            // Idle holds (default); everything else goes to Idle via Wild.
            op_case(
                r_d_after_case,
                r_q_state,
                vec![
                    (CaseArgument::Slot(lit_idle), r_d_default),
                    (CaseArgument::Wild, r_arm_to_idle),
                ],
            ),
        ];
        let return_slot = wrap_return(&mut ops, 6, r_d_after_case, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert!(result.unanalyzable.is_empty());
        let mut t = result.transitions.clone();
        t.sort();
        assert_eq!(
            t,
            vec![
                Transition { source_index: 0, target_index: 0 }, // Idle → Idle (held)
                Transition { source_index: 1, target_index: 0 }, // Running → Idle (Wild)
                Transition { source_index: 2, target_index: 0 }, // Done → Idle (Wild)
            ]
        );
    }

    /// Negative test: kernel return slot is not a Tuple.  Surface a
    /// kernel-level Unanalyzable diagnostic with synthetic source
    /// "<kernel>".
    #[test]
    fn principled_non_tuple_return_yields_kernel_level_unanalyzable() {
        let r_d = make_register(0);
        let sl_dont_care = make_slot(50);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());

        // Return slot is r_d directly (not a Tuple).
        let ops = vec![op_assign(r_d, sl_dont_care)];

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            r_d,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert!(result.transitions.is_empty());
        assert_eq!(result.unanalyzable.len(), 1);
        assert_eq!(result.unanalyzable[0].0, "<kernel>");
        assert!(
            result.unanalyzable[0].1.contains("Tuple"),
            "diagnostic should mention 'Tuple'; got: {}",
            result.unanalyzable[0].1
        );
    }

    /// Negative test: Enum opcode whose discriminant matches no
    /// variant.  The walker propagates the error; per-arm
    /// Unanalyzable is surfaced.
    #[test]
    fn principled_enum_with_unknown_discriminant_yields_arm_unanalyzable() {
        let q_reg = make_register(100);
        let o_dummy = make_register(101);

        let r_d_init = make_register(0);
        let r_q_state = make_register(1);
        let r_d_default = make_register(2);
        let r_bad_enum = make_register(3); // disc=99 — not in 3-state descriptor
        let r_arm_idle = make_register(4);
        let r_d_after_case = make_register(5);
        let sl_dont_care = make_slot(50);
        let lit_idle = make_slot(51);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());
        lookup.insert(lit_idle, lit_disc_unsigned(0, 2));

        let mut ops = vec![
            op_assign(r_d_init, sl_dont_care),
            op_index(r_q_state, q_reg, state_path()),
            op_splice(r_d_default, r_d_init, state_path(), r_q_state),
            op_enum(r_bad_enum, vec![], lit_disc_unsigned(99, 8)),
            op_splice(r_arm_idle, r_d_default, state_path(), r_bad_enum),
            // Only Idle arm is malformed; other variants hold via default.
            op_case(
                r_d_after_case,
                r_q_state,
                vec![
                    (CaseArgument::Slot(lit_idle), r_arm_idle),
                    (CaseArgument::Wild, r_d_default),
                ],
            ),
        ];
        let return_slot = wrap_return(&mut ops, 6, r_d_after_case, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_descriptor(),
            &lookup_fn,
        );
        // Idle should produce Unanalyzable; Running and Done hold.
        assert_eq!(result.unanalyzable.len(), 1);
        assert_eq!(result.unanalyzable[0].0, "Idle");
        let mut t = result.transitions.clone();
        t.sort();
        assert_eq!(
            t,
            vec![
                Transition { source_index: 1, target_index: 1 }, // Running → Running (Wild → default → q.state)
                Transition { source_index: 2, target_index: 2 }, // Done → Done
            ]
        );
    }

    // =====================================================
    // Failure-mode-specific Tier-1 tests
    //
    // Each of these pins a specific algorithm behaviour that
    // would otherwise only be tested implicitly via the doc.rs
    // adversarial widgets.  Direct Tier-1 tests give localised
    // diagnostics when an extractor change breaks one of these
    // failure modes.
    // =====================================================

    /// Reset detection: the canonical RHDL reset block
    /// `if cr.reset.any() { d.<state_field> = INIT; ... }` lowers
    /// to `Select(Unary(OrReduce, Index(cr, [.reset])), d_with_reset_override, d_normal)`.
    /// The principled algorithm recognises this shape and skips
    /// the reset-override branch (per `fsm-architecture.md` §5.1
    /// property 3).  Without this, every state would have an edge
    /// to the initial state.
    ///
    /// Synthetic kernel: kernel-top default (every variant
    /// self-loops), with a reset block that overrides d.state to
    /// Idle (variant 0).  Expected result: only the self-loops
    /// from the kernel-top default; NO reset-induced edges to
    /// state 0.
    #[test]
    fn principled_skips_reset_block() {
        let cr_reg = make_register(100);
        let q_reg = make_register(101);
        let o_dummy = make_register(102);

        let r_d_init = make_register(0);
        let r_q_state = make_register(1);
        let r_d_default = make_register(2); // d.state = q.state
        let r_to_idle = make_register(3); // Enum(Idle)
        let r_d_reset = make_register(4); // d after reset override
        let r_cr_reset = make_register(5); // cr.reset (the bool field)
        let r_reset_any = make_register(6); // OrReduce(cr.reset) = .any()
        let r_d_final = make_register(7); // Select(reset_any, reset_d, default_d)
        let sl_dont_care = make_slot(50);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());

        let reset_path = Path::default().field("reset");

        let mut ops = vec![
            op_assign(r_d_init, sl_dont_care),
            op_index(r_q_state, q_reg, state_path()),
            op_splice(r_d_default, r_d_init, state_path(), r_q_state),
            op_enum(r_to_idle, vec![], lit_disc_unsigned(0, 2)),
            op_splice(r_d_reset, r_d_default, state_path(), r_to_idle),
            // The reset condition: cr.reset → OrReduce → bool.
            op_index(r_cr_reset, cr_reg, reset_path),
            crate::rhif::rhif_builder::op_unary(
                crate::rhif::spec::AluUnary::Any,
                r_reset_any,
                r_cr_reset,
            ),
            op_select(r_d_final, r_reset_any, r_d_reset, r_d_default),
        ];
        let return_slot = wrap_return(&mut ops, 8, r_d_final, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert!(result.unanalyzable.is_empty());
        let mut t = result.transitions.clone();
        t.sort();
        // Only self-loops from the kernel-top default; NO edges
        // back to state 0 (Idle) from the reset override.  If the
        // reset detection breaks, we'd see (1, 0) and (2, 0)
        // appearing here.
        assert_eq!(
            t,
            vec![
                Transition { source_index: 0, target_index: 0 },
                Transition { source_index: 1, target_index: 1 },
                Transition { source_index: 2, target_index: 2 },
            ]
        );
    }

    /// EnumDiscriminant chain: in real RHIF, a `Case` on `q.state`
    /// has its discriminant defined by an `Index` op with path
    /// `[#]` (EnumDiscriminant), which itself indexes the result
    /// of `Index(q, [.state])`.  The constraint propagation must
    /// follow this two-step Index chain to recognise the FSM
    /// transition Case.
    ///
    /// Without the chain traversal, the walker would treat the
    /// Case as having a non-state discriminant, fall back to "union
    /// all arms," and produce universal Cartesian-product
    /// over-approximation.
    #[test]
    fn principled_traverses_enum_discriminant_index_chain() {
        let q_reg = make_register(100);
        let o_dummy = make_register(101);

        let r_q_state = make_register(0); // Index(q, [.state])
        let r_q_state_disc = make_register(1); // Index(r_q_state, [#])
        let r_d_init = make_register(2);
        let r_d_default = make_register(3);
        let r_to_running = make_register(4);
        let r_to_done = make_register(5);
        let r_to_idle = make_register(6);
        let r_arm_idle = make_register(7);
        let r_arm_running = make_register(8);
        let r_arm_done = make_register(9);
        let r_d_after_case = make_register(10);
        let sl_dont_care = make_slot(50);
        let lit_idle = make_slot(51);
        let lit_running = make_slot(52);
        let lit_done = make_slot(53);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());
        lookup.insert(lit_idle, lit_disc_unsigned(0, 2));
        lookup.insert(lit_running, lit_disc_unsigned(1, 2));
        lookup.insert(lit_done, lit_disc_unsigned(2, 2));

        let disc_path = Path::default().discriminant();

        let mut ops = vec![
            op_index(r_q_state, q_reg, state_path()),
            // The `#` extraction — real RHIF inserts this in front of every Case on an enum.
            op_index(r_q_state_disc, r_q_state, disc_path),
            op_assign(r_d_init, sl_dont_care),
            op_splice(r_d_default, r_d_init, state_path(), r_q_state),
            op_enum(r_to_running, vec![], lit_disc_unsigned(1, 2)),
            op_enum(r_to_done, vec![], lit_disc_unsigned(2, 2)),
            op_enum(r_to_idle, vec![], lit_disc_unsigned(0, 2)),
            op_splice(r_arm_idle, r_d_default, state_path(), r_to_running),
            op_splice(r_arm_running, r_d_default, state_path(), r_to_done),
            op_splice(r_arm_done, r_d_default, state_path(), r_to_idle),
            // Case discriminant is r_q_state_disc, NOT r_q_state directly.
            op_case(
                r_d_after_case,
                r_q_state_disc,
                vec![
                    (CaseArgument::Slot(lit_idle), r_arm_idle),
                    (CaseArgument::Slot(lit_running), r_arm_running),
                    (CaseArgument::Slot(lit_done), r_arm_done),
                ],
            ),
        ];
        let return_slot = wrap_return(&mut ops, 11, r_d_after_case, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert!(result.unanalyzable.is_empty());
        let mut t = result.transitions.clone();
        t.sort();
        // Constraint propagation must work even with the EnumDiscriminant
        // chain in front of the Case discriminant.  If it breaks, we'd
        // see all 9 (3x3) edges instead of the expected 3.
        assert_eq!(
            t,
            vec![
                Transition { source_index: 0, target_index: 1 },
                Transition { source_index: 1, target_index: 2 },
                Transition { source_index: 2, target_index: 0 },
            ]
        );
    }

    /// Locate-step: the kernel's d-component is a Splice chain
    /// where the most recent <state_field> override happens after
    /// several non-state Splices.  The locate-step must walk back
    /// through every non-state Splice to find the state-field
    /// Splice — verifies the chain-walk in `locate_state_field_slot`.
    #[test]
    fn principled_locate_step_walks_through_non_state_splices() {
        let q_reg = make_register(100);
        let o_dummy = make_register(101);

        let r_d_init = make_register(0);
        let r_q_state = make_register(1);
        let r_d_with_state = make_register(2); // d.state = q.state
        let r_other_value = make_register(3);
        let r_d_with_other = make_register(4); // d.other_field = X (state untouched)
        let sl_dont_care = make_slot(50);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());

        let other_path = Path::default().field("other_field");

        let mut ops = vec![
            op_assign(r_d_init, sl_dont_care),
            op_index(r_q_state, q_reg, state_path()),
            op_splice(r_d_with_state, r_d_init, state_path(), r_q_state),
            op_assign(r_other_value, sl_dont_care),
            // Splice on a different field, AFTER the state-field splice.
            // The locate-step must walk through this to find the state-field Splice.
            op_splice(r_d_with_other, r_d_with_state, other_path, r_other_value),
        ];
        let return_slot = wrap_return(&mut ops, 5, r_d_with_other, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert!(result.unanalyzable.is_empty());
        // All variants self-loop from the kernel-top default.
        let mut t = result.transitions.clone();
        t.sort();
        assert_eq!(
            t,
            vec![
                Transition { source_index: 0, target_index: 0 },
                Transition { source_index: 1, target_index: 1 },
                Transition { source_index: 2, target_index: 2 },
            ]
        );
    }

    /// Locate-step failure: the kernel's d-component chain never
    /// overrides the state field.  Per the locator's contract, this
    /// surfaces a kernel-level Unanalyzable diagnostic.  Distinct
    /// from the non-Tuple-return failure mode.
    #[test]
    fn principled_locate_failure_when_state_field_never_overridden() {
        let o_dummy = make_register(101);

        let r_d_init = make_register(0);
        let sl_dont_care = make_slot(50);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());

        // d is just dont_care, no Splices — state field never set.
        let mut ops = vec![op_assign(r_d_init, sl_dont_care)];
        let return_slot = wrap_return(&mut ops, 1, r_d_init, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert!(result.transitions.is_empty());
        assert_eq!(result.unanalyzable.len(), 1);
        assert_eq!(result.unanalyzable[0].0, "<kernel>");
        assert!(
            result.unanalyzable[0].1.contains("never overrides")
                || result.unanalyzable[0].1.contains("state field"),
            "diagnostic should explain the locate failure; got: {}",
            result.unanalyzable[0].1
        );
    }

    /// Pinning the **implicit-hold-masks-deadlock** acceptance gap
    /// documented in `fsm-architecture.md` §5.4 "Known acceptance
    /// gaps".  This test demonstrates the gap by construction:
    /// a state with NO explicit transitions and NO explicit
    /// self-loop (no `d.state = q.state` written inside any arm
    /// for this state) still ends up with a self-loop in the
    /// extracted graph because of the canonical kernel-top default
    /// `d.state = q.state`.
    ///
    /// **What the gap means.** The Layer 2 analysis pass's
    /// `DeadlockCandidate` diagnostic (`fsm::analysis`) cannot fire
    /// for such a state — the implicit self-loop makes it look
    /// like the author intended a stay-in-place behaviour, when in
    /// fact they may have just forgotten to wire transitions out.
    /// This is a real correctness concern (deadlocks ship
    /// undetected) and is named as a NECESSARY follow-up, not an
    /// optional refinement.
    ///
    /// **Synthetic construction.** Three states (Idle, Running,
    /// Done).  Kernel-top default writes `d.state = q.state`.  Then
    /// a Case on `q.state` where:
    ///   - Idle arm: `d.state = Running` (transition out)
    ///   - Running arm: empty (no body, no d.state write)
    ///       ← intended deadlock that becomes implicit self-loop
    ///   - Done arm: `d.state = Idle` (transition out)
    ///
    /// The extractor produces (Idle → Running, Running → Running,
    /// Done → Idle).  The Running → Running edge IS the masking:
    /// to a downstream deadlock check, Running looks like it has
    /// an outgoing edge (to itself), so it isn't flagged.
    ///
    /// When the follow-up lands (track explicit vs implicit
    /// self-loops separately), the extractor's output should
    /// distinguish these so the analysis layer can flag the
    /// implicit-only case.  When that happens, this test will
    /// need updating to reflect the new contract.
    #[test]
    fn principled_implicit_hold_masks_deadlock_state() {
        let q_reg = make_register(100);
        let o_dummy = make_register(101);

        let r_d_init = make_register(0);
        let r_q_state = make_register(1);
        let r_d_default = make_register(2);
        let r_to_running = make_register(3);
        let r_to_idle = make_register(4);
        let r_arm_idle = make_register(5);
        let r_arm_done = make_register(6);
        let r_d_after_case = make_register(7);
        let sl_dont_care = make_slot(50);
        let lit_idle = make_slot(51);
        let lit_running = make_slot(52);
        let lit_done = make_slot(53);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());
        lookup.insert(lit_idle, lit_disc_unsigned(0, 2));
        lookup.insert(lit_running, lit_disc_unsigned(1, 2));
        lookup.insert(lit_done, lit_disc_unsigned(2, 2));

        let mut ops = vec![
            op_assign(r_d_init, sl_dont_care),
            op_index(r_q_state, q_reg, state_path()),
            op_splice(r_d_default, r_d_init, state_path(), r_q_state),
            op_enum(r_to_running, vec![], lit_disc_unsigned(1, 2)),
            op_enum(r_to_idle, vec![], lit_disc_unsigned(0, 2)),
            op_splice(r_arm_idle, r_d_default, state_path(), r_to_running),
            op_splice(r_arm_done, r_d_default, state_path(), r_to_idle),
            // The deadlock-y arm: Running maps to r_d_default
            // (the kernel-top default), meaning no transition
            // happens and the implicit self-loop fires.
            op_case(
                r_d_after_case,
                r_q_state,
                vec![
                    (CaseArgument::Slot(lit_idle), r_arm_idle),
                    (CaseArgument::Slot(lit_running), r_d_default),
                    (CaseArgument::Slot(lit_done), r_arm_done),
                ],
            ),
        ];
        let return_slot = wrap_return(&mut ops, 8, r_d_after_case, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert!(result.unanalyzable.is_empty());
        let mut t = result.transitions.clone();
        t.sort();
        // Running → Running is the implicit self-loop that masks
        // the deadlock.  Until the spec's NECESSARY follow-up
        // lands (explicit vs implicit self-loop tracking), the
        // analysis layer cannot distinguish this from an
        // intentional self-loop, so DeadlockCandidate cannot fire.
        assert_eq!(
            t,
            vec![
                Transition { source_index: 0, target_index: 1 }, // Idle → Running (explicit)
                Transition { source_index: 1, target_index: 1 }, // Running → Running (IMPLICIT — masks deadlock)
                Transition { source_index: 2, target_index: 0 }, // Done → Idle (explicit)
            ]
        );
    }

    /// Select constraint propagation: when the Select condition is
    /// `q.<state_field> == StateX`, only the matching branch
    /// contributes per the source-variant constraint.  This test
    /// verifies the over-approximation budget §5.4 #5 doesn't
    /// loosen for the `q.<state_field> == X` pattern inside
    /// transition logic (per §5.4.2 #1 fix).
    ///
    /// Synthetic kernel:
    ///   d.state = q.state                    // kernel-top default
    ///   if q.state == Idle { d.state = Running }     // matches only when source=Idle
    ///
    /// Without constraint propagation: every variant would get an
    /// edge to Running (union of both branches).
    /// With constraint propagation: only Idle → Running, plus
    /// implicit self-loops for Running and Done.
    #[test]
    fn principled_select_constraint_propagation_on_state_eq() {
        let q_reg = make_register(100);
        let o_dummy = make_register(101);

        let r_d_init = make_register(0);
        let r_q_state = make_register(1);
        let r_d_default = make_register(2);
        let r_to_running = make_register(3);
        let r_d_taken = make_register(4);
        let r_idle_lit = make_register(5);
        let r_eq_idle = make_register(6); // q.state == Idle
        let r_d_after_select = make_register(7);
        let sl_dont_care = make_slot(50);
        let lit_idle = make_slot(51);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());
        lookup.insert(lit_idle, lit_disc_unsigned(0, 2));

        let mut ops = vec![
            op_assign(r_d_init, sl_dont_care),
            op_index(r_q_state, q_reg, state_path()),
            op_splice(r_d_default, r_d_init, state_path(), r_q_state),
            op_enum(r_to_running, vec![], lit_disc_unsigned(1, 2)),
            op_splice(r_d_taken, r_d_default, state_path(), r_to_running),
            op_enum(r_idle_lit, vec![], lit_disc_unsigned(0, 2)),
            // q.state == Idle: should resolve true only when source=Idle.
            crate::rhif::rhif_builder::op_binary(
                crate::rhif::spec::AluBinary::Eq,
                r_eq_idle,
                r_q_state,
                r_idle_lit,
            ),
            op_select(r_d_after_select, r_eq_idle, r_d_taken, r_d_default),
        ];
        let return_slot = wrap_return(&mut ops, 8, r_d_after_select, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert!(result.unanalyzable.is_empty());
        let mut t = result.transitions.clone();
        t.sort();
        // Only Idle → Running (constraint resolves true), plus
        // implicit self-loops for Running and Done (constraint
        // resolves false → false-branch is the kernel-top default).
        // Pre-fix: Running and Done would also have edges to
        // Running because we'd union both branches.
        assert_eq!(
            t,
            vec![
                Transition { source_index: 0, target_index: 1 }, // Idle → Running (resolved true)
                Transition { source_index: 1, target_index: 1 }, // Running → Running (resolved false → default)
                Transition { source_index: 2, target_index: 2 }, // Done → Done (resolved false → default)
            ]
        );
    }

    /// Companion test: the comparison's literal operand is on the
    /// LEFT side (`Idle == q.state`), not the right.  The
    /// constraint propagation must work regardless of operand order.
    #[test]
    fn principled_select_constraint_propagation_handles_swapped_operands() {
        let q_reg = make_register(100);
        let o_dummy = make_register(101);

        let r_d_init = make_register(0);
        let r_q_state = make_register(1);
        let r_d_default = make_register(2);
        let r_to_done = make_register(3);
        let r_d_taken = make_register(4);
        let r_running_lit = make_register(5);
        let r_eq_swapped = make_register(6);
        let r_d_after_select = make_register(7);
        let sl_dont_care = make_slot(50);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());

        let mut ops = vec![
            op_assign(r_d_init, sl_dont_care),
            op_index(r_q_state, q_reg, state_path()),
            op_splice(r_d_default, r_d_init, state_path(), r_q_state),
            op_enum(r_to_done, vec![], lit_disc_unsigned(2, 2)),
            op_splice(r_d_taken, r_d_default, state_path(), r_to_done),
            op_enum(r_running_lit, vec![], lit_disc_unsigned(1, 2)),
            // Running == q.state — operand order swapped.
            crate::rhif::rhif_builder::op_binary(
                crate::rhif::spec::AluBinary::Eq,
                r_eq_swapped,
                r_running_lit,
                r_q_state,
            ),
            op_select(r_d_after_select, r_eq_swapped, r_d_taken, r_d_default),
        ];
        let return_slot = wrap_return(&mut ops, 8, r_d_after_select, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert!(result.unanalyzable.is_empty());
        let mut t = result.transitions.clone();
        t.sort();
        assert_eq!(
            t,
            vec![
                Transition { source_index: 0, target_index: 0 }, // Idle hold (false)
                Transition { source_index: 1, target_index: 2 }, // Running → Done (true)
                Transition { source_index: 2, target_index: 2 }, // Done hold (false)
            ]
        );
    }

    /// Negative test: when the Select condition is opaque (e.g.,
    /// reads an input bool or an arithmetic expression), constraint
    /// propagation correctly bails and falls back to union of both
    /// branches.  This prevents the Select-handler from
    /// false-positively constraining when the condition isn't
    /// actually a state-eq comparison.
    #[test]
    fn principled_select_constraint_propagation_falls_back_on_opaque_cond() {
        let q_reg = make_register(100);
        let o_dummy = make_register(101);
        let opaque_cond = make_register(102); // some unrelated bool input

        let r_d_init = make_register(0);
        let r_q_state = make_register(1);
        let r_d_default = make_register(2);
        let r_to_running = make_register(3);
        let r_d_taken = make_register(4);
        let r_d_after_select = make_register(5);
        let sl_dont_care = make_slot(50);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());

        let mut ops = vec![
            op_assign(r_d_init, sl_dont_care),
            op_index(r_q_state, q_reg, state_path()),
            op_splice(r_d_default, r_d_init, state_path(), r_q_state),
            op_enum(r_to_running, vec![], lit_disc_unsigned(1, 2)),
            op_splice(r_d_taken, r_d_default, state_path(), r_to_running),
            // Opaque condition (function arg) — not a state-eq comparison.
            op_select(r_d_after_select, opaque_cond, r_d_taken, r_d_default),
        ];
        let return_slot = wrap_return(&mut ops, 6, r_d_after_select, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert!(result.unanalyzable.is_empty());
        let mut t = result.transitions.clone();
        t.sort();
        // Every variant gets an edge to Running (true-branch) AND
        // its self-loop (false-branch).  Sound over-approximation.
        assert_eq!(
            t,
            vec![
                Transition { source_index: 0, target_index: 0 },
                Transition { source_index: 0, target_index: 1 },
                Transition { source_index: 1, target_index: 1 },
                Transition { source_index: 2, target_index: 1 },
                Transition { source_index: 2, target_index: 2 },
            ]
        );
    }

    // =====================================================
    // allow_implicit = false (strict mode) tests
    //
    // Pin the new opt-in behaviour: when the widget descriptor
    // sets allow_implicit=false, implicit self-loops disappear
    // from the graph.  States whose only would-be outgoing edge
    // was an implicit self-loop end up with no outgoing edges,
    // surfacing as DeadlockCandidate in the analysis layer.
    //
    // This closes `fsm-architecture.md` §5.4.1.
    // =====================================================

    /// Strict mode: kernel-top default alone yields NO transitions
    /// (vs `allow_implicit=true` which yields 3 self-loops).  Every
    /// variant has zero outgoing edges; the analysis layer would
    /// fire DeadlockCandidate for each.
    #[test]
    fn strict_mode_kernel_top_default_alone_yields_no_transitions() {
        let q_reg = make_register(100);
        let o_dummy = make_register(101);

        let r_d_init = make_register(0);
        let r_q_state = make_register(1);
        let r_d_default = make_register(2);
        let sl_dont_care = make_slot(50);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());

        let mut ops = vec![
            op_assign(r_d_init, sl_dont_care),
            op_index(r_q_state, q_reg, state_path()),
            op_splice(r_d_default, r_d_init, state_path(), r_q_state),
        ];
        let return_slot = wrap_return(&mut ops, 3, r_d_default, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_strict_descriptor(),
            &lookup_fn,
        );
        assert!(result.unanalyzable.is_empty());
        // Strict: NO transitions.  Analysis layer sees zero
        // outgoing edges per variant → DeadlockCandidate fires.
        assert_eq!(
            result.transitions,
            Vec::<Transition>::new(),
            "strict mode should produce no transitions for kernel-top default alone; \
             got {:?}",
            result.transitions
        );
    }

    /// Strict mode: a guarded transition with implicit-hold
    /// else-branch produces ONLY the explicit edge, not the
    /// implicit self-loop.  The else-branch's "no state write"
    /// stays "no contribution" — the analysis layer will see the
    /// missing self-loop edge and won't be misled into thinking
    /// the state has an intentional stay-in-place behaviour.
    ///
    /// This is the can_master `Id`-arm shape verbatim, but with
    /// allow_implicit=false instead of true.
    #[test]
    fn strict_mode_guarded_transition_emits_only_explicit_edge() {
        let q_reg = make_register(100);
        let o_dummy = make_register(101);
        let cond_reg = make_register(102);

        let r_d_init = make_register(0);
        let r_q_state = make_register(1);
        let r_d_default = make_register(2);
        let r_to_running = make_register(3);
        let r_d_taken = make_register(4);
        let r_arm_idle = make_register(5);
        let r_d_after_case = make_register(6);
        let sl_dont_care = make_slot(50);
        let lit_idle = make_slot(51);
        let lit_running = make_slot(52);
        let lit_done = make_slot(53);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());
        lookup.insert(lit_idle, lit_disc_unsigned(0, 2));
        lookup.insert(lit_running, lit_disc_unsigned(1, 2));
        lookup.insert(lit_done, lit_disc_unsigned(2, 2));

        let mut ops = vec![
            op_assign(r_d_init, sl_dont_care),
            op_index(r_q_state, q_reg, state_path()),
            op_splice(r_d_default, r_d_init, state_path(), r_q_state),
            op_enum(r_to_running, vec![], lit_disc_unsigned(1, 2)),
            op_splice(r_d_taken, r_d_default, state_path(), r_to_running),
            op_select(r_arm_idle, cond_reg, r_d_taken, r_d_default),
            op_case(
                r_d_after_case,
                r_q_state,
                vec![
                    (CaseArgument::Slot(lit_idle), r_arm_idle),
                    (CaseArgument::Slot(lit_running), r_d_default),
                    (CaseArgument::Slot(lit_done), r_d_default),
                ],
            ),
        ];
        let return_slot = wrap_return(&mut ops, 7, r_d_after_case, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_strict_descriptor(),
            &lookup_fn,
        );
        assert!(result.unanalyzable.is_empty());
        let mut t = result.transitions.clone();
        t.sort();
        // Strict: ONLY the explicit Idle → Running edge.  No
        // implicit self-loops on Idle (else branch) nor on
        // Running / Done (their arms hold via default).  The
        // Running and Done states have no outgoing edges →
        // DeadlockCandidate fires for both in the analysis layer.
        assert_eq!(
            t,
            vec![
                Transition { source_index: 0, target_index: 1 }, // Idle → Running (explicit)
            ]
        );
    }

    /// Strict mode + explicit self-loop is preserved.  When the
    /// kernel author writes `d.state = q.state` *inside an arm*
    /// (not just at the kernel top), that's an explicit self-loop
    /// and IS included in the graph regardless of allow_implicit.
    /// The data-flow walk reaches the Index-on-q.<state_field>
    /// terminal, which under allow_implicit=false returns empty
    /// — so even an explicit `d.state = q.state` inside an arm
    /// disappears in strict mode.  This is the design's
    /// trade-off: "explicit self-loop" requires writing
    /// `d.state = State::A` (as a literal) inside arm A, not
    /// `d.state = q.state`.  Documented here.
    ///
    /// (A future enhancement could track explicit-via-literal
    /// vs implicit-via-default separately and treat them
    /// distinctly.  See `fsm-architecture.md` §5.4.1.)
    #[test]
    fn strict_mode_explicit_self_loop_via_literal_is_preserved() {
        let q_reg = make_register(100);
        let o_dummy = make_register(101);

        let r_d_init = make_register(0);
        let r_q_state = make_register(1);
        let r_d_default = make_register(2);
        let r_to_idle = make_register(3); // Enum(Idle) — the literal self-loop
        let r_arm_idle = make_register(4);
        let r_d_after_case = make_register(5);
        let sl_dont_care = make_slot(50);
        let lit_idle = make_slot(51);

        let mut lookup = BTreeMap::new();
        lookup.insert(sl_dont_care, lit_d_dont_care());
        lookup.insert(lit_idle, lit_disc_unsigned(0, 2));

        let mut ops = vec![
            op_assign(r_d_init, sl_dont_care),
            op_index(r_q_state, q_reg, state_path()),
            op_splice(r_d_default, r_d_init, state_path(), r_q_state),
            op_enum(r_to_idle, vec![], lit_disc_unsigned(0, 2)),
            // Idle arm explicitly writes d.state = Idle (via literal).
            op_splice(r_arm_idle, r_d_default, state_path(), r_to_idle),
            // Wild catches Running and Done (which under strict
            // mode contribute nothing — they hold via the
            // kernel-top default but the implicit self-loop
            // doesn't fire).
            op_case(
                r_d_after_case,
                r_q_state,
                vec![
                    (CaseArgument::Slot(lit_idle), r_arm_idle),
                    (CaseArgument::Wild, r_d_default),
                ],
            ),
        ];
        let return_slot = wrap_return(&mut ops, 6, r_d_after_case, o_dummy);

        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            return_slot,
            &three_state_strict_descriptor(),
            &lookup_fn,
        );
        assert!(result.unanalyzable.is_empty());
        // Only Idle → Idle: the explicit Splice with r_to_idle
        // (an Enum literal) writes the state field explicitly.
        // Running and Done (Wild arm) hold via the kernel-top
        // default — under strict mode their implicit self-loops
        // are NOT included.  The analysis layer would fire
        // DeadlockCandidate for Running and Done.
        assert_eq!(
            result.transitions,
            vec![Transition { source_index: 0, target_index: 0 }]
        );
    }

    // Suppress unused-import warning for PathElement.
    #[allow(dead_code)]
    fn _path_element_marker(_: PathElement) {}
}
