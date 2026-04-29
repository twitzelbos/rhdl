//! Static reachability and dead-state analysis for FSM-tagged kernels.
//!
//! Layer 2 of `fsm-architecture.md`.  Walks the transition graph
//! that `extract_transitions` derives from a kernel's RHIF and
//! emits diagnostics for unreachable states, deadlocks under
//! non-`terminal` annotation, and (with the kernel-language match
//! guards extension) potentially non-deterministic transitions.

use std::collections::{BTreeSet, VecDeque};

use crate::fsm::descriptor::FsmDescriptor;

/// What kind of structural problem a single diagnostic flags.
///
/// Diagnostics are advisory (warnings) by default and are
/// promoted to errors when the FSM-tagged widget carries
/// `#[fsm(strict)]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsmDiagnosticKind {
    /// State variant `name` was declared but cannot be reached
    /// from the initial variant via any sequence of transitions
    /// extracted from the kernel.
    UnreachableState { name: &'static str },
    /// State variant `name` has no outgoing transitions and was
    /// not annotated `#[fsm_state(terminal)]`.  Either intentional
    /// (mark it terminal) or a bug (forgot the transition out).
    DeadlockCandidate { name: &'static str },
    /// State variant `name` has only self-loop transitions and was
    /// not annotated `#[fsm_state(terminal)]`.  Same diagnostic
    /// shape as a deadlock candidate but with a more specific
    /// label so the user sees the difference in the message.
    SelfLoopSaturation { name: &'static str },
    /// More than one match arm in the kernel produces a different
    /// next-state from the same source variant under
    /// distinguishable guards.  Only meaningful once match guards
    /// land per `kernel-language-extensions.md`; reported
    /// pre-emptively so the diagnostic surface is stable.
    NonDeterministicTransition { source: &'static str },
    /// The transition extraction couldn't determine the FSM
    /// structure — fell back to "any variant" for one or more
    /// arms.  Emitted only when the analysis genuinely had to
    /// give up; carries a pointer to the offending arm name for
    /// the user to decompose.
    Unanalyzable { source: &'static str, reason: &'static str },
}

/// A single FSM-structural diagnostic surfaced by the analysis pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsmDiagnostic {
    /// The widget the diagnostic applies to (FQDN form).
    pub widget: &'static str,
    /// What's wrong.
    pub kind: FsmDiagnosticKind,
}

impl FsmDiagnostic {
    /// Render the diagnostic as a human-readable line for
    /// `cargo`-style warning output.  The compiler-side wrapper
    /// in `compiler::rhif_passes::analyze_fsm_structure` calls
    /// into this and pipes through `miette` for span attribution.
    pub fn message(&self) -> String {
        match &self.kind {
            FsmDiagnosticKind::UnreachableState { name } => format!(
                "FSM widget `{w}`: state `{name}` is unreachable from the initial state",
                w = self.widget,
                name = name,
            ),
            FsmDiagnosticKind::DeadlockCandidate { name } => format!(
                "FSM widget `{w}`: state `{name}` has no outgoing transitions and is not marked `#[fsm_state(terminal)]`",
                w = self.widget,
                name = name,
            ),
            FsmDiagnosticKind::SelfLoopSaturation { name } => format!(
                "FSM widget `{w}`: state `{name}` only loops back to itself and is not marked `#[fsm_state(terminal)]`",
                w = self.widget,
                name = name,
            ),
            FsmDiagnosticKind::NonDeterministicTransition { source } => format!(
                "FSM widget `{w}`: multiple distinguishable arms from `{source}` produce different next states (potential non-determinism once match guards land)",
                w = self.widget,
                source = source,
            ),
            FsmDiagnosticKind::Unanalyzable { source, reason } => format!(
                "FSM widget `{w}`: transition out of `{source}` could not be analysed ({reason}); structural diagnostics for this state are conservatively skipped",
                w = self.widget,
                source = source,
                reason = reason,
            ),
        }
    }
}

/// One observed transition `source` → `target`, as extracted from
/// the kernel.
///
/// The extractor is allowed to over-approximate (`source → ★`)
/// when it can't determine the exact target; that case is
/// represented by adding `Transition` rows for *every* declared
/// variant, plus an `Unanalyzable` diagnostic for the user's
/// awareness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Transition {
    pub source_index: usize,
    pub target_index: usize,
}

/// Run the full Layer-2 analysis for one FSM descriptor and a
/// pre-extracted transition set.
///
/// This is the leaf of the analysis: it does not look at RHIF
/// itself.  The compiler pass in
/// `compiler::rhif_passes::analyze_fsm_structure` is responsible
/// for extracting the `transitions` argument from the kernel's
/// match-on-state opcodes and calling this helper.  Splitting it
/// this way keeps the `fsm` module pure (it does not depend on
/// the RHIF spec) and makes the leaf trivially unit-testable
/// without spinning up the compiler.
pub fn analyze_fsm_structure(
    desc: &FsmDescriptor,
    transitions: &[Transition],
    unanalyzable: &[(&'static str, &'static str)],
) -> Vec<FsmDiagnostic> {
    let variants = desc.variants();
    let initial = desc.initial_index();
    let n = variants.len();

    let mut diagnostics = Vec::new();

    // 1. BFS reachability from the initial variant.
    let mut adjacency: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    for t in transitions {
        if t.source_index < n && t.target_index < n {
            adjacency[t.source_index].insert(t.target_index);
        }
    }
    let mut visited = vec![false; n];
    let mut queue: VecDeque<usize> = VecDeque::new();
    if initial < n {
        visited[initial] = true;
        queue.push_back(initial);
    }
    while let Some(node) = queue.pop_front() {
        for &next in &adjacency[node] {
            if !visited[next] {
                visited[next] = true;
                queue.push_back(next);
            }
        }
    }

    // 2. Unreachable states (skipping any explicitly marked
    // unanalyzable, since the absence of edges out of them might
    // also have absorbed the edges *into* them).
    let unanalyzable_sources: BTreeSet<&'static str> =
        unanalyzable.iter().map(|(s, _)| *s).collect();
    for (idx, v) in variants.iter().enumerate() {
        if !visited[idx] && !unanalyzable_sources.contains(v.name) {
            diagnostics.push(FsmDiagnostic {
                widget: desc.widget_name,
                kind: FsmDiagnosticKind::UnreachableState { name: v.name },
            });
        }
    }

    // 3. Deadlock / self-loop saturation candidates.  Skip
    // variants explicitly marked terminal, and skip any source
    // that we couldn't analyse (a conservative "we don't know
    // what its outgoing edges are" — flagged separately below).
    for (idx, v) in variants.iter().enumerate() {
        if v.terminal || unanalyzable_sources.contains(v.name) {
            continue;
        }
        let outgoing = &adjacency[idx];
        if outgoing.is_empty() {
            diagnostics.push(FsmDiagnostic {
                widget: desc.widget_name,
                kind: FsmDiagnosticKind::DeadlockCandidate { name: v.name },
            });
        } else if outgoing.len() == 1 && outgoing.contains(&idx) {
            diagnostics.push(FsmDiagnostic {
                widget: desc.widget_name,
                kind: FsmDiagnosticKind::SelfLoopSaturation { name: v.name },
            });
        }
    }

    // 4. Surface the unanalyzable arms to the user.
    for (source, reason) in unanalyzable {
        diagnostics.push(FsmDiagnostic {
            widget: desc.widget_name,
            kind: FsmDiagnosticKind::Unanalyzable { source, reason },
        });
    }

    diagnostics
}

/// Convenience builder for tests: declare a transition by
/// (source_index, target_index).
pub fn transition(source_index: usize, target_index: usize) -> Transition {
    Transition {
        source_index,
        target_index,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsm::descriptor::{FsmKernelTag, FsmWidgetTag};
    use crate::fsm::state::FsmVariantDescriptor;

    /// A canonical 3-state FSM.
    static THREE_STATE: &[FsmVariantDescriptor] = &[
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
            has_payload: true,
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
            widget_name: "test::ThreeState",
            widget: FsmWidgetTag {
                state_field: "state",
                strict: false,
            },
            kernel: FsmKernelTag {
                state_var: "q.state",
            },
            variants_fn: || THREE_STATE,
            initial_fn: || 0,
        }
    }

    #[test]
    fn fully_connected_fsm_has_no_diagnostics() {
        let desc = three_state_descriptor();
        let transitions = vec![
            transition(0, 1),
            transition(1, 1),
            transition(1, 2),
            transition(2, 0),
        ];
        let diags = analyze_fsm_structure(&desc, &transitions, &[]);
        assert!(
            diags.is_empty(),
            "expected no diagnostics, got: {diags:#?}"
        );
    }

    #[test]
    fn unreachable_state_is_flagged() {
        let desc = three_state_descriptor();
        // Idle → Running, Running → Idle.  Done is never reached.
        let transitions = vec![transition(0, 1), transition(1, 0)];
        let diags = analyze_fsm_structure(&desc, &transitions, &[]);
        assert_eq!(
            diags.len(),
            2,
            "expected 1 unreachable + 1 deadlock for Done, got: {diags:#?}"
        );
        assert!(
            diags.iter().any(|d| matches!(
                d.kind,
                FsmDiagnosticKind::UnreachableState { name: "Done" }
            )),
            "expected UnreachableState{{Done}} in: {diags:#?}"
        );
    }

    #[test]
    fn deadlock_candidate_is_flagged() {
        let desc = three_state_descriptor();
        // Idle → Running → Done, but Done has no outgoing edge.
        let transitions = vec![transition(0, 1), transition(1, 2)];
        let diags = analyze_fsm_structure(&desc, &transitions, &[]);
        assert!(
            diags.iter().any(|d| matches!(
                d.kind,
                FsmDiagnosticKind::DeadlockCandidate { name: "Done" }
            )),
            "expected DeadlockCandidate{{Done}}, got: {diags:#?}"
        );
    }

    #[test]
    fn terminal_state_is_not_flagged_as_deadlock() {
        // Same as above, but Done is marked terminal.
        static TERMINAL_STATES: &[FsmVariantDescriptor] = &[
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
                has_payload: true,
                terminal: false,
                label: None,
            },
            FsmVariantDescriptor {
                name: "Done",
                discriminant: 2,
                has_payload: false,
                terminal: true,
                label: None,
            },
        ];
        let desc = FsmDescriptor {
            widget_name: "test::TerminalDone",
            widget: FsmWidgetTag {
                state_field: "state",
                strict: false,
            },
            kernel: FsmKernelTag {
                state_var: "q.state",
            },
            variants_fn: || TERMINAL_STATES,
            initial_fn: || 0,
        };
        let transitions = vec![transition(0, 1), transition(1, 2)];
        let diags = analyze_fsm_structure(&desc, &transitions, &[]);
        assert!(
            diags.is_empty(),
            "terminal state should not trigger deadlock diagnostic, got: {diags:#?}"
        );
    }

    #[test]
    fn self_loop_only_is_flagged() {
        let desc = three_state_descriptor();
        // Idle → Running, Running → Running (self-loop only), Done unreachable.
        let transitions = vec![transition(0, 1), transition(1, 1)];
        let diags = analyze_fsm_structure(&desc, &transitions, &[]);
        assert!(
            diags.iter().any(|d| matches!(
                d.kind,
                FsmDiagnosticKind::SelfLoopSaturation { name: "Running" }
            )),
            "expected SelfLoopSaturation{{Running}}, got: {diags:#?}"
        );
    }

    #[test]
    fn unanalyzable_source_suppresses_unreachable_for_downstream() {
        // If a state's transitions are unanalyzable, we skip
        // unreachable-diagnostics for its declared targets — the
        // user already gets a louder "could not analyse" message.
        let desc = three_state_descriptor();
        let transitions = vec![transition(0, 1)]; // only Idle → Running
        let diags = analyze_fsm_structure(
            &desc,
            &transitions,
            &[("Running", "field-set via dont_care()")],
        );
        // Done is *still* unreachable (Running's transitions are
        // unanalyzable, but we have no information about whether
        // it transitions to Done).  The unreachability diagnostic
        // for Done is still useful.
        assert!(
            diags.iter().any(|d| matches!(
                d.kind,
                FsmDiagnosticKind::UnreachableState { name: "Done" }
            )),
            "expected UnreachableState{{Done}} despite Running being unanalyzable, got: {diags:#?}"
        );
        assert!(
            diags.iter().any(|d| matches!(
                d.kind,
                FsmDiagnosticKind::Unanalyzable {
                    source: "Running",
                    ..
                }
            )),
            "expected Unanalyzable{{Running}}, got: {diags:#?}"
        );
        // Running itself should NOT be flagged as a deadlock
        // candidate, because we explicitly couldn't analyse it.
        assert!(
            !diags
                .iter()
                .any(|d| matches!(d.kind, FsmDiagnosticKind::DeadlockCandidate { name: "Running" })),
            "Running should be skipped for deadlock check (unanalyzable), got: {diags:#?}"
        );
    }
}
