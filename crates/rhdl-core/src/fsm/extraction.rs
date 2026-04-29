//! Extract the FSM transition graph from a kernel's RHIF.
//!
//! Layer 2's analysis pass needs to know, for each FSM-tagged
//! kernel, the set of `(source variant, target variant)` pairs
//! the kernel's match-on-state can produce.  This module is the
//! extractor: it walks the RHIF opcodes, recognises the
//! canonical FSM idiom, and emits the extracted transitions in
//! the format that [`super::analysis::analyze_fsm_structure`]
//! consumes.
//!
//! ## What "canonical" means
//!
//! The extractor recognises the pattern:
//!
//! ```ignore
//! let next = match q.<state_field> {
//!     State::A => /* expr that constructs State::B */,
//!     State::B => /* expr that constructs State::C */,
//!     ...
//! };
//! d.<state_field> = next;
//! ```
//!
//! Mechanically this lowers to:
//! - An `Index` on `q` with `path = [<state_field>]` producing the
//!   state's slot.  The state's discriminant is then the input to
//!   a `Case` opcode.
//! - One arm per source variant, each with a result slot whose
//!   defining opcode is an `Enum` constructing the target
//!   variant.
//!
//! For arms whose result isn't a simple `Enum` (e.g., the next
//! state is computed across multiple sub-paths), the extractor
//! reports the source variant as `unanalyzable` and the leaf
//! analysis will skip over it for the deadlock check.
//!
//! ## Implicit self-loops
//!
//! Production RHDL widgets follow CLAUDE.md §3's "construct via
//! `dont_care()`, then assign every meaningful field" pattern,
//! which for FSM widgets typically means writing
//! `d.<state_field> = q.<state_field>` once at the top of the
//! kernel and only overriding it in arms that actually
//! transition.  An arm with no `d.<state_field>` write — or a
//! conditional inside an arm whose else-branch quietly omits the
//! assignment — is therefore *not* a bug; it means "hold the
//! state in place this cycle."
//!
//! The extractor encodes this convention: when both walkers
//! (value-form and side-effect-form) run cleanly but find no
//! state-overriding op for an arm, the arm is interpreted as a
//! self-loop on its source variant.  Only when a walker hits a
//! genuine error (an enum template with no resolvable
//! discriminant, etc.) does the extractor surface
//! `Unanalyzable`.  This makes the canonical CAN/SPI/UART/I²C
//! kernel shape (guarded transitions with implicit hold
//! else-branches) extractable without per-widget rewrites.
//!
//! ## Limitations (v1)
//!
//! - Only the immediate-defining opcode of an arm result is
//!   inspected.  Nested `if/else` (which lowers to additional
//!   `Case` or `Select` opcodes) is conservatively flagged as
//!   unanalyzable.  Once Layer 5 (BMC) lands, the extractor can
//!   be extended to do deeper case analysis.
//! - Match arms with payload bindings (`State::Running { counter
//!   } => ...`) are recognised; the binding is ignored when
//!   determining the source variant.
//! - The extractor does not yet handle transitions encoded as
//!   field-by-field assignment to a `dont_care()`-constructed
//!   state struct (the "non-canonical" pattern called out in
//!   `fsm-architecture.md` §10).  Such patterns produce a single
//!   `Unanalyzable` diagnostic for the entire kernel.

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

/// Walk a slot's data-flow graph backward to find every state
/// variant the slot can possibly hold *as a state-typed value*.
///
/// Handles:
/// - Literal of the state type (the "no-payload variant" case).
/// - `OpCode::Enum` whose template encodes a known discriminant.
/// - `OpCode::Assign` (forwarded — recurse on rhs).
/// - `OpCode::Select` (if-else expression — union of both branches).
/// - `OpCode::Case` (a nested match — union of all arm results).
/// - `OpCode::Index` reading `<some_q>.<state_field>` — yields
///   `[hint_self_loop_to]` if a hint is supplied (the value is the
///   state at the start of the arm we're inside, i.e., the matched
///   variant), else empty.
///
/// Returns `Ok(set of variant indices)`, deduplicated and sorted.
/// `Err((source_name, reason))` is reserved for cases the walker
/// cannot make any sense of (used by the caller to emit an
/// `Unanalyzable` diagnostic).  An empty `Ok` set means "the slot
/// doesn't define a state value here, but the walker isn't
/// confused" — typically used when the slot's defining op is
/// just plumbing the state through unchanged.
fn variants_in_state_value_slot(
    desc: &FsmDescriptor,
    ops: &[OpCode],
    slot: Slot,
    state_field: &str,
    literal_lookup: &impl Fn(Slot) -> Option<TypedBits>,
    hint_self_loop_to: Option<usize>,
) -> Result<Vec<usize>, &'static str> {
    // Literal of the state type — direct discriminant lookup.
    if let Some(tb) = literal_lookup(slot) {
        if let Some(disc) = typed_bits_to_discriminant(&tb) {
            if let Some(idx) = variant_index_for_discriminant(desc, disc) {
                return Ok(vec![idx]);
            }
        }
    }
    let Some(definer) = find_definer(ops, slot) else {
        return Ok(vec![]);
    };
    match definer {
        OpCode::Enum(e) => {
            let disc = typed_bits_to_discriminant(&e.template)
                .ok_or("enum template has no resolvable discriminant")?;
            let idx = variant_index_for_discriminant(desc, disc)
                .ok_or("enum discriminant doesn't match any variant")?;
            Ok(vec![idx])
        }
        OpCode::Assign(a) => {
            variants_in_state_value_slot(desc, ops, a.rhs, state_field, literal_lookup, hint_self_loop_to)
        }
        OpCode::Select(sel) => {
            let mut t = variants_in_state_value_slot(
                desc, ops, sel.true_value, state_field, literal_lookup, hint_self_loop_to,
            )?;
            let f = variants_in_state_value_slot(
                desc, ops, sel.false_value, state_field, literal_lookup, hint_self_loop_to,
            )?;
            for v in f {
                if !t.contains(&v) {
                    t.push(v);
                }
            }
            t.sort();
            Ok(t)
        }
        OpCode::Case(case) => {
            // A nested match — union of every arm's result.
            let mut all = Vec::new();
            for (_arg, arm_slot) in &case.table {
                let arm = variants_in_state_value_slot(
                    desc, ops, *arm_slot, state_field, literal_lookup, hint_self_loop_to,
                )?;
                for v in arm {
                    if !all.contains(&v) {
                        all.push(v);
                    }
                }
            }
            all.sort();
            Ok(all)
        }
        OpCode::Index(idx) if path_targets_state_field(&idx.path, state_field) => {
            // Reading `.state` of some struct (typically `q`).  If we
            // have a self-loop hint, it points at the matched variant
            // of the arm we're inside.
            Ok(hint_self_loop_to.into_iter().collect())
        }
        // Anything else producing a state-typed slot is opaque.
        _ => Ok(vec![]),
    }
}

/// Walk a slot's data-flow graph backward to find every state
/// variant the `state_field` of that slot can hold.  This is the
/// side-effect-style walker for kernels whose match arms produce
/// a `D`-struct as the arm result rather than a state value
/// directly.
///
/// Handles the canonical lowering shape:
/// - `OpCode::Splice { orig, path, subst }` where `path = [.state]`:
///   The state field was overwritten with `subst`.  Recurse via
///   [`variants_in_state_value_slot`] on `subst`.
/// - `OpCode::Splice` with a different path: state field unchanged
///   from `orig`; recurse on `orig`.
/// - `OpCode::Select`: union both branches.
/// - `OpCode::Assign`: forwarded; recurse on rhs.
/// - `OpCode::Struct`: explicit struct construction; if the named
///   `state_field` appears in the field list, recurse on its slot;
///   otherwise the field comes from the template (typically
///   `dont_care()`) and we yield empty.
fn variants_in_d_state_field(
    desc: &FsmDescriptor,
    ops: &[OpCode],
    slot: Slot,
    state_field: &str,
    literal_lookup: &impl Fn(Slot) -> Option<TypedBits>,
    hint_self_loop_to: Option<usize>,
) -> Result<Vec<usize>, &'static str> {
    // Helper: at union points (Select branches, Case arms), any
    // sub-walk that finds no state-overriding op means "this path
    // held the state in place" → self-loop on the source variant.
    // The hint carries the source variant index from the enclosing
    // arm so we can emit the self-loop without re-deriving it.
    // We deliberately do NOT apply this at leaf return points
    // (find_definer-None, unrecognised opcode, Struct-without-state-field):
    // those are caught by the top-level fallback in
    // [`extract_canonical_transitions`] so they don't pollute
    // value-form analyses (e.g., the d-struct walker called on an
    // Enum slot must return empty, not self-loop, so the value
    // walker's interpretation wins).
    let self_loop_or_empty = || -> Vec<usize> { hint_self_loop_to.into_iter().collect() };
    let Some(definer) = find_definer(ops, slot) else {
        return Ok(vec![]);
    };
    match definer {
        OpCode::Splice(s) => {
            if path_targets_state_field(&s.path, state_field) {
                variants_in_state_value_slot(
                    desc, ops, s.subst, state_field, literal_lookup, hint_self_loop_to,
                )
            } else {
                variants_in_d_state_field(
                    desc, ops, s.orig, state_field, literal_lookup, hint_self_loop_to,
                )
            }
        }
        OpCode::Select(sel) => {
            // Walk both branches.  For each branch that finds no
            // state-overriding op, treat it as a self-loop
            // contribution (per the implicit-self-loop convention)
            // — silently dropping the empty branch would lose the
            // "this branch held state in place" information at the
            // union point.
            let mut t = variants_in_d_state_field(
                desc, ops, sel.true_value, state_field, literal_lookup, hint_self_loop_to,
            )?;
            if t.is_empty() {
                t = self_loop_or_empty();
            }
            let mut f = variants_in_d_state_field(
                desc, ops, sel.false_value, state_field, literal_lookup, hint_self_loop_to,
            )?;
            if f.is_empty() {
                f = self_loop_or_empty();
            }
            for v in f {
                if !t.contains(&v) {
                    t.push(v);
                }
            }
            t.sort();
            Ok(t)
        }
        OpCode::Assign(a) => variants_in_d_state_field(
            desc, ops, a.rhs, state_field, literal_lookup, hint_self_loop_to,
        ),
        OpCode::Case(case) => {
            // Nested match producing a D struct.  Union all arms;
            // each empty arm contributes a self-loop per the
            // implicit-self-loop convention.
            let mut all = Vec::new();
            for (_arg, arm_slot) in &case.table {
                let mut arm = variants_in_d_state_field(
                    desc, ops, *arm_slot, state_field, literal_lookup, hint_self_loop_to,
                )?;
                if arm.is_empty() {
                    arm = self_loop_or_empty();
                }
                for v in arm {
                    if !all.contains(&v) {
                        all.push(v);
                    }
                }
            }
            all.sort();
            Ok(all)
        }
        OpCode::Struct(s) => {
            for fv in &s.fields {
                if let Member::Named(name) = &fv.member {
                    if name.as_str() == state_field {
                        return variants_in_state_value_slot(
                            desc, ops, fv.value, state_field, literal_lookup, hint_self_loop_to,
                        );
                    }
                }
            }
            // Field not in explicit list — comes from template
            // (e.g., dont_care()).  Return empty; the top-level
            // fallback applies the self-loop interpretation if
            // the value-form walker also returns empty.
            Ok(vec![])
        }
        // Anything else (e.g., a slot defined by an Enum, Add, or
        // any opcode the d-struct walker doesn't recognise) is
        // opaque w.r.t. d-struct interpretation — return empty
        // and let the value-form walker's analysis win at the top
        // level if it has a state-value reading of this slot.
        _ => Ok(vec![]),
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

/// Extract transitions from a sequence of RHIF opcodes for one
/// FSM-tagged kernel.
///
/// `ops` is the kernel's full op list.  `desc` is the FSM
/// descriptor for this widget.  `literal_lookup` resolves
/// `Slot::Literal` slots to their underlying `TypedBits` (the
/// caller wires this up from the kernel `Object`'s symbol table).
///
/// The current heuristic recognises the first `Case` opcode
/// whose discriminant is reached by a single-step `Index` from
/// the kernel argument named `q` with a path matching the
/// state-field name in `desc`.  This is the canonical FSM
/// pattern; other shapes produce no transitions and a single
/// "could not analyze" diagnostic.
pub fn extract_canonical_transitions(
    ops: &[OpCode],
    desc: &FsmDescriptor,
    literal_lookup: LiteralLookup<'_>,
) -> ExtractionResult {
    let mut result = ExtractionResult::default();

    // Find the first Case opcode.  In v1 we assume there's exactly
    // one match-on-state per kernel; multi-match kernels are out of
    // scope for this iteration of Layer 2.
    let case = ops.iter().find_map(|op| match op {
        OpCode::Case(c) => Some(c),
        _ => None,
    });

    let Some(case) = case else {
        // No `match` at all in this kernel — nothing to analyze.
        // We don't emit an Unanalyzable diagnostic for this since
        // the user's widget just doesn't have a state-machine
        // shape (could be a pure dataflow kernel).  Return empty.
        return result;
    };

    let state_field = desc.widget.state_field;

    for (arg, result_slot) in &case.table {
        let source = source_variant_for_case_arg(desc, arg, &literal_lookup);
        let Some(source_idx) = source else {
            // Wild arm or unresolvable case argument.  Wild arms
            // commonly cover unmatched variants — we don't add an
            // Unanalyzable diagnostic here because the leaf
            // analysis would also see the wild arm as "any
            // target".  Skip silently.
            continue;
        };
        let source_name = desc.variants()[source_idx].name;
        let hint = Some(source_idx);

        // Try interpretation 1: the arm's result slot IS the new
        // state value (let-binding form: `let next = match ... { A => B }`).
        let value_path = variants_in_state_value_slot(
            desc,
            ops,
            *result_slot,
            state_field,
            &literal_lookup,
            hint,
        );
        // Try interpretation 2: the arm's result slot is a D struct
        // and the state was updated via Splice (side-effect form:
        // `match ... { A => { d.state = B; } }`).
        let d_path = variants_in_d_state_field(
            desc,
            ops,
            *result_slot,
            state_field,
            &literal_lookup,
            hint,
        );

        let mut found: Vec<usize> = Vec::new();
        let mut errors: Vec<&'static str> = Vec::new();
        match value_path {
            Ok(vs) => {
                for v in vs {
                    if !found.contains(&v) {
                        found.push(v);
                    }
                }
            }
            Err(e) => errors.push(e),
        }
        match d_path {
            Ok(vs) => {
                for v in vs {
                    if !found.contains(&v) {
                        found.push(v);
                    }
                }
            }
            Err(e) => errors.push(e),
        }

        if found.is_empty() {
            if errors.is_empty() {
                // Both walkers ran cleanly but found no
                // state-overriding op in this arm.  Per RHDL's
                // canonical "construct via dont_care(), then
                // assign every meaningful field" pattern
                // (CLAUDE.md §3), an FSM kernel typically writes
                // `d.<state_field> = q.<state_field>` at the top
                // and only overrides it in arms that transition.
                // An arm that produces no state override therefore
                // means "hold the state in place" — emit a
                // self-loop on the source variant.  This handles
                // the most common real-world widget shape, where
                // a guard's else-branch quietly omits the d.state
                // assignment (e.g., `if guard { d.state = X } else
                // { d.other = ... }`).
                result.transitions.push(Transition {
                    source_index: source_idx,
                    target_index: source_idx,
                });
            } else {
                // A walker hit a real error (e.g., an enum
                // template with no resolvable discriminant).
                // Surface it — silence here would hide a real bug.
                let reason = errors.first().copied().unwrap();
                result.unanalyzable.push((source_name, reason));
            }
        } else {
            for target_idx in found {
                result.transitions.push(Transition {
                    source_index: source_idx,
                    target_index: target_idx,
                });
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
    use crate::rhif::rhif_builder::{op_assign, op_case, op_enum};
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
        FsmDescriptor {
            widget_name: "test::Three",
            widget: FsmWidgetTag {
                state_field: "state",
                strict: false,
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

    #[test]
    fn extracts_three_simple_transitions() {
        // Synthetic kernel:
        //   r0 <- enum(disc=1)        // State::Running
        //   r1 <- enum(disc=2)        // State::Done
        //   r2 <- enum(disc=0)        // State::Idle
        //   r3 <- case(state, [
        //       (lit_disc=0) => r0,   // Idle    → Running
        //       (lit_disc=1) => r1,   // Running → Done
        //       (lit_disc=2) => r2,   // Done    → Idle
        //   ])
        let r0 = make_register(0);
        let r1 = make_register(1);
        let r2 = make_register(2);
        let r3 = make_register(3);
        let state = make_register(4); // the discriminant slot
        let lit0 = make_slot(0);
        let lit1 = make_slot(1);
        let lit2 = make_slot(2);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));
        lookup.insert(lit1, lit_disc_unsigned(1, 2));
        lookup.insert(lit2, lit_disc_unsigned(2, 2));

        let ops = vec![
            op_enum(r0, vec![], lit_disc_unsigned(1, 2)),
            op_enum(r1, vec![], lit_disc_unsigned(2, 2)),
            op_enum(r2, vec![], lit_disc_unsigned(0, 2)),
            op_case(
                r3,
                state,
                vec![
                    (CaseArgument::Slot(lit0), r0),
                    (CaseArgument::Slot(lit1), r1),
                    (CaseArgument::Slot(lit2), r2),
                ],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert_eq!(result.unanalyzable.len(), 0, "got: {:?}", result.unanalyzable);
        let mut transitions = result.transitions.clone();
        transitions.sort();
        assert_eq!(
            transitions,
            vec![
                Transition { source_index: 0, target_index: 1 },
                Transition { source_index: 1, target_index: 2 },
                Transition { source_index: 2, target_index: 0 },
            ]
        );
    }

    #[test]
    fn no_match_kernel_yields_no_transitions() {
        // A kernel without a Case opcode is not an FSM kernel.
        // Extractor should emit zero transitions and zero
        // unanalyzable diagnostics.
        let r0 = make_register(0);
        let r1 = make_register(1);
        let ops = vec![op_assign(r0, r1)];
        let lookup_fn = make_lookup(BTreeMap::new());
        let result = extract_canonical_transitions(
            &ops,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert_eq!(result.transitions.len(), 0);
        assert_eq!(result.unanalyzable.len(), 0);
    }

    #[test]
    fn arm_with_no_recognisable_target_yields_implicit_self_loop() {
        // One arm targets a recognisable Enum (Idle → Running), the
        // other targets a slot whose definer isn't found in the op
        // list (Running → ???).  Per the implicit-self-loop
        // convention, the latter is interpreted as Running → Running
        // (the kernel did not produce a state-overriding op for that
        // arm, so the state holds in place).  No Unanalyzable
        // diagnostic — that is reserved for genuinely malformed IR
        // (see [`arm_with_malformed_enum_template_yields_unanalyzable`]).
        let r0 = make_register(0);
        let r1 = make_register(1);
        let r2 = make_register(2);
        let r3 = make_register(3); // no defining op: implicit self-loop
        let state = make_register(4);
        let lit0 = make_slot(0);
        let lit1 = make_slot(1);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));
        lookup.insert(lit1, lit_disc_unsigned(1, 2));

        let ops = vec![
            op_enum(r0, vec![], lit_disc_unsigned(1, 2)),
            op_enum(r1, vec![], lit_disc_unsigned(2, 2)),
            op_case(
                r2,
                state,
                vec![
                    (CaseArgument::Slot(lit0), r0),
                    (CaseArgument::Slot(lit1), r3),
                ],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            &three_state_descriptor(),
            &lookup_fn,
        );
        let mut t = result.transitions.clone();
        t.sort();
        assert_eq!(
            t,
            vec![
                Transition { source_index: 0, target_index: 1 }, // Idle → Running (explicit)
                Transition { source_index: 1, target_index: 1 }, // Running → Running (implicit self-loop)
            ]
        );
        assert!(
            result.unanalyzable.is_empty(),
            "got: {:?}",
            result.unanalyzable
        );
    }

    #[test]
    fn assign_forwarding_is_traced_through() {
        // r1 = Enum(disc=2)
        // r2 = Assign(r1)       // forwarded
        // case [ (Slot(lit0)) => r2 ]  // Idle → Done via assign
        let r1 = make_register(1);
        let r2 = make_register(2);
        let r3 = make_register(3);
        let state = make_register(4);
        let lit0 = make_slot(0);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));

        let ops = vec![
            op_enum(r1, vec![], lit_disc_unsigned(2, 2)),
            op_assign(r2, r1),
            op_case(
                r3,
                state,
                vec![(CaseArgument::Slot(lit0), r2)],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            &three_state_descriptor(),
            &lookup_fn,
        );
        assert_eq!(result.transitions.len(), 1);
        assert_eq!(result.transitions[0].source_index, 0);
        assert_eq!(result.transitions[0].target_index, 2);
    }

    #[test]
    fn wild_arms_are_skipped_silently() {
        let r0 = make_register(0);
        let r3 = make_register(3);
        let state = make_register(4);
        let lit0 = make_slot(0);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));

        let ops = vec![
            op_enum(r0, vec![], lit_disc_unsigned(1, 2)),
            op_case(
                r3,
                state,
                vec![
                    (CaseArgument::Slot(lit0), r0),
                    (CaseArgument::Wild, r0),
                ],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result = extract_canonical_transitions(
            &ops,
            &three_state_descriptor(),
            &lookup_fn,
        );
        // Only the explicit (lit0 → r0) arm produces a transition.
        // The wild arm is skipped.
        assert_eq!(result.transitions.len(), 1);
    }

    // ---------------------------------------------------------------
    // Adversarial test matrix for the side-effect-form walker
    // (PR `feat/fsm-extractor-side-effects`).  Each test covers a
    // distinct kernel-language construct that lowers into a
    // particular RHIF shape the walker must handle correctly.
    // ---------------------------------------------------------------

    use crate::rhif::rhif_builder::{op_select, op_splice, op_struct};
    use crate::rhif::spec::{FieldValue, Member};
    use crate::types::path::PathElement;

    /// Build a `Path` referencing a single named field — the canonical
    /// path produced by `d.state = ...` lowering.
    fn state_path() -> Path {
        Path::default().field("state")
    }

    /// Build a typed-bits literal of the D struct (a stub — we only
    /// need it for the literal-table; the extractor never reads its
    /// value when it's the rest-template of a Struct opcode).
    fn lit_d_dont_care() -> TypedBits {
        // We use the same width-2 bits stub as the discriminant
        // literals.  The extractor only cares whether the discriminant
        // path resolves; the rest-template here is opaque.
        lit_disc_unsigned(0, 2)
    }

    /// A descriptor whose `state_field` matches what the new walker
    /// expects.  Reuses THREE's variants.
    fn three_state_d_descriptor() -> FsmDescriptor {
        three_state_descriptor()
    }

    /// (A1) Side-effect form: each arm splices a literal into d.state.
    ///   r0 = enum(Running)
    ///   r1 = enum(Done)
    ///   r2 = enum(Idle)
    ///   r3 = D::dont_care()             (literal sl_d)
    ///   r4 = splice(r3, .state, r0)      // for arm Idle
    ///   r5 = splice(r3, .state, r1)      // for arm Running
    ///   r6 = splice(r3, .state, r2)      // for arm Done
    ///   r7 = case(state, [Idle=>r4, Running=>r5, Done=>r6])
    #[test]
    fn side_effect_form_three_unconditional_splices() {
        let r0 = make_register(0);
        let r1 = make_register(1);
        let r2 = make_register(2);
        let r3 = make_register(3);
        let r4 = make_register(4);
        let r5 = make_register(5);
        let r6 = make_register(6);
        let r7 = make_register(7);
        let state = make_register(8);
        let lit0 = make_slot(0);
        let lit1 = make_slot(1);
        let lit2 = make_slot(2);
        let sl_d = make_slot(3);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));
        lookup.insert(lit1, lit_disc_unsigned(1, 2));
        lookup.insert(lit2, lit_disc_unsigned(2, 2));
        lookup.insert(sl_d, lit_d_dont_care());

        let ops = vec![
            op_enum(r0, vec![], lit_disc_unsigned(1, 2)),
            op_enum(r1, vec![], lit_disc_unsigned(2, 2)),
            op_enum(r2, vec![], lit_disc_unsigned(0, 2)),
            op_assign(r3, sl_d),
            op_splice(r4, r3, state_path(), r0),
            op_splice(r5, r3, state_path(), r1),
            op_splice(r6, r3, state_path(), r2),
            op_case(
                r7,
                state,
                vec![
                    (CaseArgument::Slot(lit0), r4),
                    (CaseArgument::Slot(lit1), r5),
                    (CaseArgument::Slot(lit2), r6),
                ],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result =
            extract_canonical_transitions(&ops, &three_state_d_descriptor(), &lookup_fn);
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
                Transition { source_index: 0, target_index: 1 },
                Transition { source_index: 1, target_index: 2 },
                Transition { source_index: 2, target_index: 0 },
            ]
        );
    }

    /// (A2) Side-effect default + conditional override:
    ///   for arm Idle: r_default = splice(r_d, .state, r_q_state)   // d.state = q.state
    ///                 r_taken   = splice(r_d, .state, r_running)   // d.state = Running
    ///                 arm_result = select(cond, r_taken, r_default)
    /// Should yield Idle→Idle (self-loop via q.state) AND Idle→Running.
    #[test]
    fn side_effect_with_default_then_conditional_override_yields_self_loop_plus_target() {
        let r_running = make_register(0);
        let r_d = make_register(1);
        let r_q_state = make_register(2);
        let r_default = make_register(3);
        let r_taken = make_register(4);
        let r_select = make_register(5);
        let r_case = make_register(6);
        let q_register = make_register(7);
        let cond = make_register(8);
        let state = make_register(9);
        let lit0 = make_slot(0);
        let sl_d = make_slot(1);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));
        lookup.insert(sl_d, lit_d_dont_care());

        let ops = vec![
            op_enum(r_running, vec![], lit_disc_unsigned(1, 2)),
            op_assign(r_d, sl_d),
            // r_q_state = q.state (an Index from q)
            crate::rhif::rhif_builder::op_index(r_q_state, q_register, state_path()),
            op_splice(r_default, r_d, state_path(), r_q_state),
            op_splice(r_taken, r_d, state_path(), r_running),
            op_select(r_select, cond, r_taken, r_default),
            op_case(
                r_case,
                state,
                vec![(CaseArgument::Slot(lit0), r_select)],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result =
            extract_canonical_transitions(&ops, &three_state_d_descriptor(), &lookup_fn);
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
                Transition { source_index: 0, target_index: 0 }, // Idle → Idle (self-loop)
                Transition { source_index: 0, target_index: 1 }, // Idle → Running
            ]
        );
    }

    /// (B1) Nested if-else inside an arm body:
    ///   match q.state { Idle => if c1 { d.state = Running } else if c2 { d.state = Done } }
    /// Lowers to nested Selects.  Walker should yield {Idle, Running, Done}.
    #[test]
    fn nested_if_else_in_side_effect_arm_unions_all_branches() {
        let r_running = make_register(0);
        let r_done = make_register(1);
        let r_d = make_register(2);
        let r_q_state = make_register(3);
        let r_default = make_register(4);
        let r_to_running = make_register(5);
        let r_to_done = make_register(6);
        let r_inner_select = make_register(7);
        let r_outer_select = make_register(8);
        let r_case = make_register(9);
        let q_register = make_register(10);
        let cond1 = make_register(11);
        let cond2 = make_register(12);
        let state = make_register(13);
        let lit0 = make_slot(0);
        let sl_d = make_slot(1);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));
        lookup.insert(sl_d, lit_d_dont_care());

        let ops = vec![
            op_enum(r_running, vec![], lit_disc_unsigned(1, 2)),
            op_enum(r_done, vec![], lit_disc_unsigned(2, 2)),
            op_assign(r_d, sl_d),
            crate::rhif::rhif_builder::op_index(r_q_state, q_register, state_path()),
            op_splice(r_default, r_d, state_path(), r_q_state),
            op_splice(r_to_running, r_d, state_path(), r_running),
            op_splice(r_to_done, r_d, state_path(), r_done),
            // inner = if c2 { to_done } else { default }
            op_select(r_inner_select, cond2, r_to_done, r_default),
            // outer = if c1 { to_running } else { inner }
            op_select(r_outer_select, cond1, r_to_running, r_inner_select),
            op_case(
                r_case,
                state,
                vec![(CaseArgument::Slot(lit0), r_outer_select)],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result =
            extract_canonical_transitions(&ops, &three_state_d_descriptor(), &lookup_fn);
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
                Transition { source_index: 0, target_index: 0 }, // self-loop
                Transition { source_index: 0, target_index: 1 }, // → Running
                Transition { source_index: 0, target_index: 2 }, // → Done
            ]
        );
    }

    /// (C1) Splice into a different field preserves the state field.
    ///   r_d = D::dont_care()
    ///   r_with_state = splice(r_d, .state, r_running)
    ///   r_with_other = splice(r_with_state, .other_field, r_x)
    ///   case [ Idle => r_with_other ]
    /// Walker should still report Idle→Running because .state was set
    /// before the unrelated splice.
    #[test]
    fn splice_into_unrelated_field_preserves_state_walker_result() {
        let r_running = make_register(0);
        let r_x = make_register(1);
        let r_d = make_register(2);
        let r_with_state = make_register(3);
        let r_with_other = make_register(4);
        let r_case = make_register(5);
        let state = make_register(6);
        let lit0 = make_slot(0);
        let sl_d = make_slot(1);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));
        lookup.insert(sl_d, lit_d_dont_care());

        let other_path = Path::default().field("other_field");

        let ops = vec![
            op_enum(r_running, vec![], lit_disc_unsigned(1, 2)),
            op_assign(r_x, r_running), // dummy
            op_assign(r_d, sl_d),
            op_splice(r_with_state, r_d, state_path(), r_running),
            op_splice(r_with_other, r_with_state, other_path, r_x),
            op_case(
                r_case,
                state,
                vec![(CaseArgument::Slot(lit0), r_with_other)],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result =
            extract_canonical_transitions(&ops, &three_state_d_descriptor(), &lookup_fn);
        assert!(
            result.unanalyzable.is_empty(),
            "got: {:?}",
            result.unanalyzable
        );
        assert_eq!(
            result.transitions,
            vec![Transition { source_index: 0, target_index: 1 }]
        );
    }

    /// (C2) Two splices into .state in the same arm — last write wins.
    ///   r_with_running = splice(r_d, .state, r_running)
    ///   r_with_done    = splice(r_with_running, .state, r_done)
    ///   case [ Idle => r_with_done ]
    /// Walker should report Idle→Done only (the last splice).
    #[test]
    fn back_to_back_splices_into_state_last_write_wins() {
        let r_running = make_register(0);
        let r_done = make_register(1);
        let r_d = make_register(2);
        let r_with_running = make_register(3);
        let r_with_done = make_register(4);
        let r_case = make_register(5);
        let state = make_register(6);
        let lit0 = make_slot(0);
        let sl_d = make_slot(1);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));
        lookup.insert(sl_d, lit_d_dont_care());

        let ops = vec![
            op_enum(r_running, vec![], lit_disc_unsigned(1, 2)),
            op_enum(r_done, vec![], lit_disc_unsigned(2, 2)),
            op_assign(r_d, sl_d),
            op_splice(r_with_running, r_d, state_path(), r_running),
            op_splice(r_with_done, r_with_running, state_path(), r_done),
            op_case(
                r_case,
                state,
                vec![(CaseArgument::Slot(lit0), r_with_done)],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result =
            extract_canonical_transitions(&ops, &three_state_d_descriptor(), &lookup_fn);
        assert!(result.unanalyzable.is_empty());
        assert_eq!(
            result.transitions,
            vec![Transition { source_index: 0, target_index: 2 }]
        );
    }

    /// (D1) Arm result built via explicit Struct opcode with .state field.
    #[test]
    fn struct_opcode_with_explicit_state_field_resolves() {
        let r_running = make_register(0);
        let r_struct = make_register(1);
        let r_case = make_register(2);
        let state = make_register(3);
        let lit0 = make_slot(0);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));

        let ops = vec![
            op_enum(r_running, vec![], lit_disc_unsigned(1, 2)),
            op_struct(
                r_struct,
                vec![FieldValue {
                    member: Member::Named("state".to_string().into()),
                    value: r_running,
                }],
                None,
                lit_d_dont_care(),
            ),
            op_case(
                r_case,
                state,
                vec![(CaseArgument::Slot(lit0), r_struct)],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result =
            extract_canonical_transitions(&ops, &three_state_d_descriptor(), &lookup_fn);
        assert!(result.unanalyzable.is_empty());
        assert_eq!(
            result.transitions,
            vec![Transition { source_index: 0, target_index: 1 }]
        );
    }

    /// (D2) Struct opcode WITHOUT explicit state field
    /// (template-only): the d-struct walker confirms no state field
    /// override, no error.  Per the implicit-self-loop convention
    /// (the canonical kernel keeps state in place when no override
    /// is emitted), this becomes a self-loop on the arm's source
    /// variant rather than an Unanalyzable diagnostic.
    #[test]
    fn struct_opcode_without_state_field_yields_implicit_self_loop() {
        let r_struct = make_register(0);
        let r_case = make_register(1);
        let state = make_register(2);
        let lit0 = make_slot(0);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));

        let ops = vec![
            op_struct(r_struct, vec![], None, lit_d_dont_care()),
            op_case(
                r_case,
                state,
                vec![(CaseArgument::Slot(lit0), r_struct)],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result =
            extract_canonical_transitions(&ops, &three_state_d_descriptor(), &lookup_fn);
        assert!(
            result.unanalyzable.is_empty(),
            "got: {:?}",
            result.unanalyzable
        );
        assert_eq!(
            result.transitions,
            vec![Transition { source_index: 0, target_index: 0 }]
        );
    }

    /// (E1) Bare `d.state = q.state` (default + nothing else) yields
    /// a self-loop transition.
    #[test]
    fn d_state_eq_q_state_alone_yields_self_loop() {
        let r_d = make_register(0);
        let r_q_state = make_register(1);
        let r_with_state = make_register(2);
        let r_case = make_register(3);
        let q_register = make_register(4);
        let state = make_register(5);
        let lit0 = make_slot(0);
        let sl_d = make_slot(1);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));
        lookup.insert(sl_d, lit_d_dont_care());

        let ops = vec![
            op_assign(r_d, sl_d),
            crate::rhif::rhif_builder::op_index(r_q_state, q_register, state_path()),
            op_splice(r_with_state, r_d, state_path(), r_q_state),
            op_case(
                r_case,
                state,
                vec![(CaseArgument::Slot(lit0), r_with_state)],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result =
            extract_canonical_transitions(&ops, &three_state_d_descriptor(), &lookup_fn);
        assert!(result.unanalyzable.is_empty());
        assert_eq!(
            result.transitions,
            vec![Transition { source_index: 0, target_index: 0 }]
        );
    }

    /// (F1) Both walkers run cleanly but find no state-overriding
    /// op — interpreted as an implicit self-loop, not Unanalyzable.
    /// The arm result is a register defined by Add (an opaque
    /// arithmetic op).  Neither walker recognises Add as producing
    /// a state value or a d-struct override, so both return
    /// `Ok(vec![])`.  Per the implicit-self-loop convention, this
    /// is the canonical "this arm doesn't transition" shape.
    ///
    /// For the truly-malformed case where a walker hits a real
    /// error, see [`arm_with_malformed_enum_template_yields_unanalyzable`].
    #[test]
    fn opaque_arm_result_yields_implicit_self_loop() {
        let r_opaque = make_register(0);
        let r_case = make_register(1);
        let state = make_register(2);
        let lit0 = make_slot(0);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));

        let ops = vec![
            // r_opaque is defined by Add — neither walker recognises this.
            crate::rhif::rhif_builder::op_binary(
                crate::rhif::spec::AluBinary::Add,
                r_opaque,
                state,
                state,
            ),
            op_case(
                r_case,
                state,
                vec![(CaseArgument::Slot(lit0), r_opaque)],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result =
            extract_canonical_transitions(&ops, &three_state_d_descriptor(), &lookup_fn);
        assert!(
            result.unanalyzable.is_empty(),
            "got: {:?}",
            result.unanalyzable
        );
        assert_eq!(
            result.transitions,
            vec![Transition { source_index: 0, target_index: 0 }]
        );
    }

    /// (F2) Side-effect form deduplicates: the same target reached via
    /// two distinct paths in the if-else inside an arm should produce
    /// one transition entry, not two.
    #[test]
    fn side_effect_form_deduplicates_targets_across_branches() {
        let r_running = make_register(0);
        let r_d = make_register(1);
        let r_taken_a = make_register(2);
        let r_taken_b = make_register(3);
        let r_select = make_register(4);
        let r_case = make_register(5);
        let cond = make_register(6);
        let state = make_register(7);
        let lit0 = make_slot(0);
        let sl_d = make_slot(1);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));
        lookup.insert(sl_d, lit_d_dont_care());

        let ops = vec![
            op_enum(r_running, vec![], lit_disc_unsigned(1, 2)),
            op_assign(r_d, sl_d),
            op_splice(r_taken_a, r_d, state_path(), r_running),
            op_splice(r_taken_b, r_d, state_path(), r_running),
            op_select(r_select, cond, r_taken_a, r_taken_b),
            op_case(
                r_case,
                state,
                vec![(CaseArgument::Slot(lit0), r_select)],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result =
            extract_canonical_transitions(&ops, &three_state_d_descriptor(), &lookup_fn);
        assert!(result.unanalyzable.is_empty());
        assert_eq!(
            result.transitions,
            vec![Transition { source_index: 0, target_index: 1 }]
        );
    }

    /// (F3) Mixed walkers — value form and d-struct form both recognise
    /// the arm.  E.g., a `let x = State::Done; d.state = x;` pattern
    /// where the arm result happens to be the d-struct.  Ensure
    /// transitions are merged + deduplicated, no duplicates emitted.
    #[test]
    fn value_and_d_struct_walkers_unioned_without_duplicates() {
        // Synthetic arm result reachable from BOTH walkers via the same
        // target Running.  The d-struct walker traces splice→r_running.
        // The value walker (if interpreted state-typed) returns empty
        // because the arm result isn't a state-typed slot.  So we just
        // get one transition from d-struct.
        let r_running = make_register(0);
        let r_d = make_register(1);
        let r_with_state = make_register(2);
        let r_case = make_register(3);
        let state = make_register(4);
        let lit0 = make_slot(0);
        let sl_d = make_slot(1);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));
        lookup.insert(sl_d, lit_d_dont_care());

        let ops = vec![
            op_enum(r_running, vec![], lit_disc_unsigned(1, 2)),
            op_assign(r_d, sl_d),
            op_splice(r_with_state, r_d, state_path(), r_running),
            op_case(
                r_case,
                state,
                vec![(CaseArgument::Slot(lit0), r_with_state)],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result =
            extract_canonical_transitions(&ops, &three_state_d_descriptor(), &lookup_fn);
        assert!(result.unanalyzable.is_empty());
        assert_eq!(result.transitions.len(), 1);
        assert_eq!(result.transitions[0].source_index, 0);
        assert_eq!(result.transitions[0].target_index, 1);
    }

    /// (F4) Existing let-binding-form tests must still pass after the
    /// refactor — sanity check that we haven't regressed the
    /// `target_variant_for_result` → `variants_in_state_value_slot`
    /// conversion.  This is a trivial duplicate of
    /// `extracts_three_simple_transitions` to make the regression
    /// guarantee explicit in the adversarial section.
    #[test]
    fn let_binding_form_still_works_after_refactor() {
        let r0 = make_register(0);
        let r1 = make_register(1);
        let r2 = make_register(2);
        let r3 = make_register(3);
        let state = make_register(4);
        let lit0 = make_slot(0);
        let lit1 = make_slot(1);
        let lit2 = make_slot(2);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));
        lookup.insert(lit1, lit_disc_unsigned(1, 2));
        lookup.insert(lit2, lit_disc_unsigned(2, 2));

        let ops = vec![
            op_enum(r0, vec![], lit_disc_unsigned(1, 2)),
            op_enum(r1, vec![], lit_disc_unsigned(2, 2)),
            op_enum(r2, vec![], lit_disc_unsigned(0, 2)),
            op_case(
                r3,
                state,
                vec![
                    (CaseArgument::Slot(lit0), r0),
                    (CaseArgument::Slot(lit1), r1),
                    (CaseArgument::Slot(lit2), r2),
                ],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result =
            extract_canonical_transitions(&ops, &three_state_d_descriptor(), &lookup_fn);
        assert!(result.unanalyzable.is_empty());
        let mut t = result.transitions;
        t.sort();
        assert_eq!(
            t,
            vec![
                Transition { source_index: 0, target_index: 1 },
                Transition { source_index: 1, target_index: 2 },
                Transition { source_index: 2, target_index: 0 },
            ]
        );
    }

    // ===========================================================
    // Implicit-self-loop adversarial coverage (PR
    // `feat/fsm-extractor-implicit-self-loops`)
    //
    // These tests pin the new "no state-overriding op found =>
    // self-loop" semantics that fixes the can_master /
    // i2c_master / spi_master class of widget where a guarded
    // transition's else-branch silently omits the d.state write.
    // The kernel-top default `d.<state_field> = q.<state_field>`
    // makes the omission well-defined RHDL code; the extractor
    // honours that convention.
    // ===========================================================

    /// Canonical CAN-master shape: kernel-top default `d.field =
    /// q.field`, then the FSM match has one arm that contains a
    /// guarded transition (`if cond { d.field = X }`).  The
    /// then-branch produces an explicit transition; the
    /// else-branch produces no d.field write and must be
    /// interpreted as a self-loop.
    ///
    /// Lowering shape:
    ///   d_default = Splice(d_dont_care, [field], q.field)
    ///   d_taken   = Splice(d_default,   [field], Running)
    ///   d_arm     = Select(cond, d_taken, d_default)
    ///   case [ Idle => d_arm ]
    ///
    /// Expected: { Idle → Running (taken), Idle → Idle (held) }.
    /// Without the implicit-self-loop fix, the else-branch would
    /// recurse to the d_default, which IS a state-overriding
    /// Splice (q.field), and the value-form walker would emit a
    /// self-loop via hint_self_loop_to.  This test isn't the
    /// motivating one — see [`guarded_transition_with_implicit_else_yields_self_loop`]
    /// for that — but pins this shape too.
    #[test]
    fn kernel_top_default_plus_guarded_transition_yields_both_edges() {
        let r_running = make_register(0);
        let r_d = make_register(1);
        let r_q_field = make_register(2);
        let r_default = make_register(3);
        let r_taken = make_register(4);
        let r_select = make_register(5);
        let r_case = make_register(6);
        let q_register = make_register(7);
        let cond = make_register(8);
        let state = make_register(9);
        let lit0 = make_slot(0);
        let sl_d = make_slot(1);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));
        lookup.insert(sl_d, lit_d_dont_care());

        let ops = vec![
            op_enum(r_running, vec![], lit_disc_unsigned(1, 2)),
            op_assign(r_d, sl_d),
            crate::rhif::rhif_builder::op_index(r_q_field, q_register, state_path()),
            // Kernel-top default: d.field = q.field
            op_splice(r_default, r_d, state_path(), r_q_field),
            // Inside the arm: d.field = Running
            op_splice(r_taken, r_default, state_path(), r_running),
            // if cond { d.field = Running } else { /* d.field unchanged */ }
            op_select(r_select, cond, r_taken, r_default),
            op_case(
                r_case,
                state,
                vec![(CaseArgument::Slot(lit0), r_select)],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result =
            extract_canonical_transitions(&ops, &three_state_d_descriptor(), &lookup_fn);
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
                Transition { source_index: 0, target_index: 0 }, // Idle → Idle (else-branch)
                Transition { source_index: 0, target_index: 1 }, // Idle → Running (then-branch)
            ]
        );
    }

    /// The motivating real-world shape: a guarded transition where
    /// the else-branch writes to a *different* field (e.g.,
    /// `d.field_bit_idx`) but does NOT touch `d.field`.  This is
    /// the can_master `Id` arm verbatim: `if bit_idx == 10 {
    /// d.field = Rtr; d.field_bit_idx = 0 } else { d.field_bit_idx
    /// = next_idx }`.
    ///
    /// Lowering shape (no kernel-top d.field default — the d
    /// going into the arm comes from upstream Splices that don't
    /// touch field):
    ///   d_in     = (some unrelated splice chain, no state field)
    ///   d_taken  = Splice(d_in, [field], Running)
    ///   d_other  = Splice(d_in, [other_field], <something>)
    ///   d_arm    = Select(cond, d_taken, d_other)
    ///   case [ Idle => d_arm ]
    ///
    /// The else-branch recurses to d_other → recurses to d_in,
    /// which has no state-field Splice anywhere.  Walker returns
    /// Ok(empty).  Per the implicit-self-loop convention, this
    /// becomes Idle → Idle.
    #[test]
    fn guarded_transition_with_implicit_else_yields_self_loop() {
        use crate::types::path::Path;
        let r_running = make_register(0);
        let r_d_in = make_register(1);
        let r_other_value = make_register(2);
        let r_taken = make_register(3);
        let r_other = make_register(4);
        let r_select = make_register(5);
        let r_case = make_register(6);
        let cond = make_register(7);
        let state = make_register(8);
        let lit0 = make_slot(0);
        let sl_d = make_slot(1);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));
        lookup.insert(sl_d, lit_d_dont_care());

        let other_path = Path::default().field("other_field");

        let ops = vec![
            op_enum(r_running, vec![], lit_disc_unsigned(1, 2)),
            op_assign(r_d_in, sl_d),
            // Some unrelated value driving d.other_field in the else-branch.
            op_assign(r_other_value, sl_d),
            // then-branch: d.field = Running (state-overriding)
            op_splice(r_taken, r_d_in, state_path(), r_running),
            // else-branch: d.other_field = something (NOT state-overriding)
            op_splice(r_other, r_d_in, other_path, r_other_value),
            op_select(r_select, cond, r_taken, r_other),
            op_case(
                r_case,
                state,
                vec![(CaseArgument::Slot(lit0), r_select)],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result =
            extract_canonical_transitions(&ops, &three_state_d_descriptor(), &lookup_fn);
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
                Transition { source_index: 0, target_index: 0 }, // Idle → Idle (else: no state write)
                Transition { source_index: 0, target_index: 1 }, // Idle → Running (then-branch)
            ]
        );
    }

    /// Bare arm with no state write at all (no Splice on state
    /// path, no kernel-top default visible to the walker).  Per
    /// the implicit-self-loop convention, this is a self-loop on
    /// the source variant.
    ///
    /// Concretely: arm result is the bare d_dont_care (e.g.,
    /// `match q.state { Idle => { d.unrelated = X } }` where the
    /// d-struct chain never touches the state field).
    #[test]
    fn arm_with_no_state_write_at_all_yields_self_loop() {
        use crate::types::path::Path;
        let r_d = make_register(0);
        let r_other_value = make_register(1);
        let r_other = make_register(2);
        let r_case = make_register(3);
        let state = make_register(4);
        let lit0 = make_slot(0);
        let sl_d = make_slot(1);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));
        lookup.insert(sl_d, lit_d_dont_care());

        let other_path = Path::default().field("other_field");

        let ops = vec![
            op_assign(r_d, sl_d),
            op_assign(r_other_value, sl_d),
            op_splice(r_other, r_d, other_path, r_other_value),
            op_case(
                r_case,
                state,
                vec![(CaseArgument::Slot(lit0), r_other)],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result =
            extract_canonical_transitions(&ops, &three_state_d_descriptor(), &lookup_fn);
        assert!(
            result.unanalyzable.is_empty(),
            "got: {:?}",
            result.unanalyzable
        );
        assert_eq!(
            result.transitions,
            vec![Transition { source_index: 0, target_index: 0 }]
        );
    }

    /// Negative test: a genuinely malformed arm — the value-form
    /// walker hits an Enum opcode whose discriminant value matches
    /// no variant in the descriptor (e.g., the kernel emits a
    /// state value the type system says shouldn't exist).  This is
    /// the path that still produces an `Unanalyzable` diagnostic
    /// after the implicit-self-loop change.  Without this
    /// distinction, every kind of malformed IR would be silently
    /// re-interpreted as a self-loop, which would mask real bugs.
    #[test]
    fn arm_with_unmatched_enum_discriminant_yields_unanalyzable() {
        let r_bad = make_register(0);
        let r_case = make_register(1);
        let state = make_register(2);
        let lit0 = make_slot(0);

        let mut lookup = BTreeMap::new();
        lookup.insert(lit0, lit_disc_unsigned(0, 2));

        let ops = vec![
            // Enum producing discriminant 99 — no variant in the
            // 3-state descriptor has that.  variant_index_for_discriminant
            // returns None and the walker's `.ok_or(...)?` propagates
            // "enum discriminant doesn't match any variant".
            op_enum(r_bad, vec![], lit_disc_unsigned(99, 8)),
            op_case(
                r_case,
                state,
                vec![(CaseArgument::Slot(lit0), r_bad)],
            ),
        ];
        let lookup_fn = make_lookup(lookup);
        let result =
            extract_canonical_transitions(&ops, &three_state_d_descriptor(), &lookup_fn);
        assert!(
            result.transitions.is_empty(),
            "got: {:?}",
            result.transitions
        );
        assert_eq!(result.unanalyzable.len(), 1);
        assert_eq!(result.unanalyzable[0].0, "Idle");
        // Pin the diagnostic message — this is what the user/LLM
        // sees and acts on.
        assert!(
            result.unanalyzable[0].1.contains("discriminant")
                || result.unanalyzable[0].1.contains("variant"),
            "diagnostic should mention 'discriminant' or 'variant'; got: {}",
            result.unanalyzable[0].1
        );
    }

    // Suppress unused-import warning for PathElement; it's part of
    // the public extraction API surface but only used implicitly via
    // `Path::field()` constructor in tests.
    #[allow(dead_code)]
    fn _path_element_marker(_: PathElement) {}
}
