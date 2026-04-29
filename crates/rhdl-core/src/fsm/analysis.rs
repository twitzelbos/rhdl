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

    // ===========================================================
    // Adversarial diagnostic-message tests
    // -----------------------------------------------------------
    // These tests assert on the *rendered text* a user (or LLM
    // agent) sees when a diagnostic is surfaced.  The surface area
    // of an FSM diagnostic isn't the `kind` enum — it's the human-
    // readable string the compiler prints.  An LLM that needs to
    // refactor a broken FSM has to be able to read the diagnostic
    // and infer:
    //
    //   1. Which widget is broken (so it can locate the source).
    //   2. Which state is the problem (so it can locate the arm).
    //   3. What the structural issue is (so it can choose a fix).
    //   4. What it should do about it (the message hints at the fix).
    //
    // The renderers in `FsmDiagnostic::message` are the contract.
    // These tests pin that contract so changes to the message text
    // surface as failing tests, not silent UX regressions.
    // ===========================================================

    #[test]
    fn diagnostic_message_for_unreachable_state_localizes_widget_and_state() {
        let desc = three_state_descriptor();
        // Done is unreachable.
        let diags = analyze_fsm_structure(&desc, &[transition(0, 1), transition(1, 0)], &[]);
        let unreach = diags
            .iter()
            .find(|d| matches!(d.kind, FsmDiagnosticKind::UnreachableState { name: "Done" }))
            .expect("expected UnreachableState{Done}");
        let msg = unreach.message();
        // Required content: widget FQDN, state name, the word
        // "unreachable", "initial state".
        assert!(
            msg.contains("test::ThreeState"),
            "missing widget FQDN: {msg}"
        );
        assert!(msg.contains("`Done`"), "missing state-name backticks: {msg}");
        assert!(msg.contains("unreachable"), "missing keyword: {msg}");
        assert!(
            msg.contains("initial state"),
            "missing reachability anchor: {msg}"
        );
    }

    #[test]
    fn diagnostic_message_for_deadlock_candidate_hints_at_fix() {
        let desc = three_state_descriptor();
        // Running has no outgoing edge.
        let diags = analyze_fsm_structure(&desc, &[transition(0, 1)], &[]);
        let dead = diags
            .iter()
            .find(|d| {
                matches!(
                    d.kind,
                    FsmDiagnosticKind::DeadlockCandidate { name: "Running" }
                )
            })
            .expect("expected DeadlockCandidate{Running}");
        let msg = dead.message();
        // The message must name the fix the user can apply.
        assert!(msg.contains("`Running`"), "missing state name: {msg}");
        assert!(
            msg.contains("no outgoing transitions"),
            "missing structural diagnosis: {msg}"
        );
        assert!(
            msg.contains("`#[fsm_state(terminal)]`"),
            "missing fix hint (the terminal annotation): {msg}"
        );
    }

    #[test]
    fn diagnostic_message_for_self_loop_saturation_distinguishes_from_deadlock() {
        let desc = three_state_descriptor();
        // Running → Running only.
        let diags = analyze_fsm_structure(
            &desc,
            &[transition(0, 1), transition(1, 1)],
            &[],
        );
        let sat = diags
            .iter()
            .find(|d| {
                matches!(
                    d.kind,
                    FsmDiagnosticKind::SelfLoopSaturation { name: "Running" }
                )
            })
            .expect("expected SelfLoopSaturation{Running}");
        let msg = sat.message();
        // The user has to be able to tell self-loop-saturation apart
        // from a plain deadlock — different fix.
        assert!(msg.contains("only loops back to itself"), "{msg}");
        assert!(msg.contains("`#[fsm_state(terminal)]`"), "{msg}");
        // The deadlock-candidate language MUST NOT appear here, so
        // a user/LLM doesn't conflate the two diagnostics.
        assert!(
            !msg.contains("no outgoing transitions"),
            "self-loop msg leaked deadlock language: {msg}"
        );
    }

    #[test]
    fn diagnostic_message_for_unanalyzable_carries_source_and_reason() {
        let desc = three_state_descriptor();
        let diags = analyze_fsm_structure(
            &desc,
            &[transition(0, 1)],
            &[(
                "Running",
                "result expression is not a recognisable enum constructor",
            )],
        );
        let un = diags
            .iter()
            .find(|d| {
                matches!(
                    d.kind,
                    FsmDiagnosticKind::Unanalyzable {
                        source: "Running",
                        ..
                    }
                )
            })
            .expect("expected Unanalyzable{Running}");
        let msg = un.message();
        assert!(msg.contains("`Running`"), "missing source variant: {msg}");
        assert!(
            msg.contains("result expression is not a recognisable enum constructor"),
            "missing extractor reason in: {msg}"
        );
        // The user has to know structural diagnostics are
        // suppressed for this state — otherwise they might think
        // there's no problem.
        assert!(
            msg.contains("conservatively skipped"),
            "missing skip-warning: {msg}"
        );
    }

    #[test]
    fn diagnostic_message_for_non_deterministic_transition_names_source() {
        let desc = three_state_descriptor();
        let diag = FsmDiagnostic {
            widget: desc.widget_name,
            kind: FsmDiagnosticKind::NonDeterministicTransition { source: "Idle" },
        };
        let msg = diag.message();
        assert!(msg.contains("`Idle`"));
        assert!(msg.contains("non-determinism") || msg.contains("non-deterministic"));
        // The diagnostic foreshadows the match-guard extension; that
        // wording should stay so users searching for "guard"-related
        // diagnostics find this one.
        assert!(msg.contains("guard"), "missing 'guard' anchor: {msg}");
    }

    #[test]
    fn multiple_concurrent_failures_each_get_their_own_diagnostic() {
        // Adversarial setup: an FSM where TWO arms are unanalyzable,
        // ONE state is unreachable, and ONE state is a deadlock
        // candidate.  All four must surface — the first failure
        // must not mask the others.
        static FOUR_STATE: &[FsmVariantDescriptor] = &[
            FsmVariantDescriptor {
                name: "Idle",
                discriminant: 0,
                has_payload: false,
                terminal: false,
                label: None,
            },
            FsmVariantDescriptor {
                name: "Working",
                discriminant: 1,
                has_payload: false,
                terminal: false,
                label: None,
            },
            FsmVariantDescriptor {
                name: "Stuck",
                discriminant: 2,
                has_payload: false,
                terminal: false,
                label: None,
            },
            FsmVariantDescriptor {
                name: "Orphan",
                discriminant: 3,
                has_payload: false,
                terminal: false,
                label: None,
            },
        ];
        let desc = FsmDescriptor {
            widget_name: "test::FourState",
            widget: FsmWidgetTag {
                state_field: "state",
                strict: false,
            },
            kernel: FsmKernelTag {
                state_var: "q.state",
            },
            variants_fn: || FOUR_STATE,
            initial_fn: || 0,
        };
        // Idle → Stuck (Stuck has no outgoing edges → deadlock).
        // Working and Orphan are unanalyzable.
        // Orphan is also unreachable, but that gets suppressed by
        // the "unanalyzable absorbs unreachability" rule? No — per
        // existing analysis.rs behaviour, ONLY the source of the
        // unanalyzable flag is excused; targets still get flagged.
        // So Orphan should still be flagged unreachable.
        // Wait — Orphan IS a source (it's in the unanalyzable list).
        // Per the existing rule (unanalyzable_sources.contains(name)
        // suppresses unreachable), Orphan should NOT be flagged.
        // So we expect: deadlock(Stuck) + unanalyzable(Working) +
        // unanalyzable(Orphan).  3 diagnostics total.
        let diags = analyze_fsm_structure(
            &desc,
            &[transition(0, 2)],
            &[
                ("Working", "kernel uses dont_care()"),
                ("Orphan", "kernel uses dont_care()"),
            ],
        );
        assert!(
            diags.iter().any(|d| matches!(
                d.kind,
                FsmDiagnosticKind::DeadlockCandidate { name: "Stuck" }
            )),
            "missing DeadlockCandidate{{Stuck}}, got: {diags:#?}"
        );
        // Working: unanalyzable surfaced as its own diagnostic.
        assert!(
            diags.iter().any(|d| matches!(
                d.kind,
                FsmDiagnosticKind::Unanalyzable { source: "Working", .. }
            )),
            "missing Unanalyzable{{Working}}, got: {diags:#?}"
        );
        // Orphan: unanalyzable surfaced as its own diagnostic; the
        // unreachable diagnostic is suppressed because Orphan is in
        // the unanalyzable-sources set.
        assert!(
            diags.iter().any(|d| matches!(
                d.kind,
                FsmDiagnosticKind::Unanalyzable { source: "Orphan", .. }
            )),
            "missing Unanalyzable{{Orphan}}, got: {diags:#?}"
        );
        assert!(
            !diags.iter().any(|d| matches!(
                d.kind,
                FsmDiagnosticKind::UnreachableState { name: "Working" }
            )),
            "Working should NOT be flagged unreachable (it's an unanalyzable source)"
        );
    }

    #[test]
    fn diagnostic_widget_field_propagates_across_kinds() {
        // Every diagnostic kind must carry the widget name so
        // multi-widget compilation output can localize.  Verify
        // each kind's renderer includes it.
        let widget = "very::deeply::nested::WidgetName";
        let kinds = [
            FsmDiagnosticKind::UnreachableState { name: "X" },
            FsmDiagnosticKind::DeadlockCandidate { name: "X" },
            FsmDiagnosticKind::SelfLoopSaturation { name: "X" },
            FsmDiagnosticKind::NonDeterministicTransition { source: "X" },
            FsmDiagnosticKind::Unanalyzable {
                source: "X",
                reason: "r",
            },
        ];
        for kind in kinds {
            let msg = FsmDiagnostic { widget, kind: kind.clone() }.message();
            assert!(
                msg.contains(widget),
                "diagnostic kind {kind:?} dropped the widget name in: {msg}"
            );
        }
    }

    #[test]
    fn unreachable_state_message_is_actionable_for_llm() {
        // LLM-readability test: the message must contain enough
        // *vocabulary* for an agent to decide what to do.  Specifically:
        //   - The keyword "unreachable" so the agent can recognise the
        //     diagnostic family.
        //   - The state name (in backticks for grep-ability).
        //   - The widget name (FQDN).
        //   - A pointer to where reachability starts ("initial state").
        //
        // If these go missing, an LLM-driven fix (e.g., "add a
        // transition into <state>") loses the cue it needs.
        let diag = FsmDiagnostic {
            widget: "w::W",
            kind: FsmDiagnosticKind::UnreachableState { name: "Goal" },
        };
        let msg = diag.message();
        for required in ["unreachable", "`Goal`", "w::W", "initial state"] {
            assert!(
                msg.contains(required),
                "missing required vocabulary `{required}` in: {msg}"
            );
        }
    }

    #[test]
    fn deadlock_candidate_message_is_actionable_for_llm() {
        let diag = FsmDiagnostic {
            widget: "w::W",
            kind: FsmDiagnosticKind::DeadlockCandidate { name: "Stuck" },
        };
        let msg = diag.message();
        for required in [
            "`Stuck`",
            "w::W",
            "no outgoing transitions",
            "`#[fsm_state(terminal)]`",
        ] {
            assert!(
                msg.contains(required),
                "missing required vocabulary `{required}` in: {msg}"
            );
        }
    }

    #[test]
    fn unanalyzable_message_includes_extractor_reason_string_unedited() {
        // The reason string comes from the extractor and is the
        // user's only clue about WHY extraction failed.  Verify the
        // analysis layer doesn't truncate, escape, or re-word it.
        let exotic_reason = "result expression is not a recognisable enum constructor";
        let diag = FsmDiagnostic {
            widget: "w::W",
            kind: FsmDiagnosticKind::Unanalyzable {
                source: "Foo",
                reason: exotic_reason,
            },
        };
        let msg = diag.message();
        assert!(
            msg.contains(exotic_reason),
            "extractor reason was modified by analysis-layer rendering: {msg}"
        );
    }
}
