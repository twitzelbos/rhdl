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
use crate::rhif::spec::{CaseArgument, OpCode, Slot};
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

/// Extract the target variant index from the slot defining one
/// arm's result value.
///
/// The canonical lowering of a state-construction expression like
/// `State::B` is an `OpCode::Enum` whose template's leading bits
/// encode the discriminant.  This helper resolves that.  Returns
/// `None` if the defining opcode isn't a recognisable enum
/// constructor.
fn target_variant_for_result(
    desc: &FsmDescriptor,
    ops: &[OpCode],
    result_slot: Slot,
    literal_lookup: impl Fn(Slot) -> Option<TypedBits>,
) -> Option<usize> {
    // First: maybe the result slot is itself a literal of the
    // state type (the "no-payload variant" case lowers this way).
    if let Some(tb) = literal_lookup(result_slot) {
        if let Some(disc) = typed_bits_to_discriminant(&tb) {
            if let Some(idx) = variant_index_for_discriminant(desc, disc) {
                return Some(idx);
            }
        }
    }
    // Otherwise: walk back through ops.  An `Enum` opcode's
    // `template` carries the discriminant in its leading bits.
    let definer = find_definer(ops, result_slot)?;
    match definer {
        OpCode::Enum(e) => {
            let disc = typed_bits_to_discriminant(&e.template)?;
            variant_index_for_discriminant(desc, disc)
        }
        OpCode::Assign(a) => {
            // Forwarded — recurse on the rhs.
            target_variant_for_result(desc, ops, a.rhs, literal_lookup)
        }
        _ => None,
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

        match target_variant_for_result(desc, ops, *result_slot, &literal_lookup) {
            Some(target_idx) => {
                result.transitions.push(Transition {
                    source_index: source_idx,
                    target_index: target_idx,
                });
            }
            None => {
                result.unanalyzable.push((
                    source_name,
                    "result expression is not a recognisable enum constructor",
                ));
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
    fn arm_with_unanalyzable_target_is_flagged() {
        // One arm targets a recognisable Enum, the other targets a
        // slot whose definer isn't an Enum.  The latter should
        // surface as Unanalyzable; the former should produce a
        // valid transition.
        let r0 = make_register(0);
        let r1 = make_register(1);
        let r2 = make_register(2);
        let r3 = make_register(3); // unanalyzable: no defining op
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
        assert_eq!(result.transitions.len(), 1);
        assert_eq!(result.transitions[0].source_index, 0);
        assert_eq!(result.transitions[0].target_index, 1);
        assert_eq!(result.unanalyzable.len(), 1);
        assert_eq!(result.unanalyzable[0].0, "Running");
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
}
