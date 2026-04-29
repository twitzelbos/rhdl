use rhdl::prelude::*;
use std::path::PathBuf;

#[doc(hidden)]
/// Useful for testing, but otherwise, probably not for end users
pub fn write_svg_as_markdown(vcd: SvgFile, name: &str, options: SvgOptions) -> anyhow::Result<()> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = path.join("doc");
    std::fs::create_dir_all(&path)?;
    let path = path.join(name);
    std::fs::write(path, format!("\n\n<p>\n{}\n</p>", vcd.to_string(&options)?))?;
    Ok(())
}

/// Auto-derive the FSM transition graph for `W` from its compiled
/// kernel and write the diagram as a markdown file (inline SVG
/// wrapped in `<p>` tags).  No author-curated `FSM_TRANSITIONS`
/// const is required — the transitions come straight from the RHIF
/// of `W::Kernel`.
///
/// This is the canonical Phase 3 helper called for by
/// `fsm-architecture.md` Phase 3 acceptance criterion #2:
/// > "The diagram is up-to-date with the source by virtue of being
/// > derived from it — no `cargo run --example` step required for
/// > rustdoc."
///
/// The example file generates a diagram once at build / example-run
/// time; the included markdown file is then read by rustdoc via
/// `#![doc = include_str!("...")]`.  The full Phase 3 endpoint
/// (auto-injection at `Descriptor::hdl()`-time, removing the need
/// for `include_str!` too) is a follow-on that builds on this
/// helper.
///
/// Errors out if the canonical extractor produces any
/// `Unanalyzable` diagnostic — surface the diagnostic so the
/// author can adjust the kernel shape (the typical fix is to
/// switch from "computed state via let-binding before assignment"
/// to a direct `match` arm).
pub fn write_fsm_diagram<W>(filename: &str) -> anyhow::Result<()>
where
    W: rhdl::core::fsm::FsmWidget + SynchronousIO,
{
    let transitions = rhdl::core::fsm::extract_widget_transitions_strict::<W>()?;
    let desc = W::fsm_descriptor();
    let diagram = rhdl::core::fsm::diagram::build_fsm_diagram(&desc, &transitions);
    let svg = rhdl::core::fsm::diagram::render_fsm_svg(&diagram);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("doc");
    std::fs::create_dir_all(&path)?;
    let path = path.join(filename);
    std::fs::write(path, format!("\n\n<p>\n{}\n</p>", svg))?;
    Ok(())
}

/// Drift-check helper: assert that the manually-curated transition
/// list agrees byte-for-byte with what the canonical extractor
/// would derive from the kernel.  Useful during the deprecation
/// window for the author-curated `FSM_TRANSITIONS` consts that
/// shipped before this auto-derive helper landed.
///
/// Calling this from a widget test guarantees the manual list
/// can't drift from reality: if the kernel changes (a transition
/// is added, removed, or retargeted) and the manual list isn't
/// updated to match, the test fails.
///
/// The comparison is order-independent (both sides are sorted).
pub fn assert_fsm_transitions_match<W>(
    manual: &[rhdl::core::fsm::analysis::Transition],
) -> anyhow::Result<()>
where
    W: rhdl::core::fsm::FsmWidget + SynchronousIO,
{
    let derived = rhdl::core::fsm::extract_widget_transitions_strict::<W>()?;
    let mut manual_sorted = manual.to_vec();
    manual_sorted.sort();
    if derived != manual_sorted {
        anyhow::bail!(
            "FSM_TRANSITIONS drift detected:\n  manual:  {manual_sorted:?}\n  derived: {derived:?}",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! End-to-end integration covering Phase 3 acceptance criteria
    //! #1 and #2: a real synchronous widget tagged with
    //! `#[derive(Fsm)] + #[derive(FsmWidget)]`, no manual
    //! `FSM_TRANSITIONS` const, and the diagram falls out of the
    //! kernel via the new helpers.
    use super::*;
    use crate::core::dff;

    /// A small three-state cycle FSM — the canonical "test the FSM
    /// machinery" enum that mirrors the synthetic three-state
    /// transition graph in the rhdl-core extractor unit tests.
    #[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
    pub enum CycleState {
        #[default]
        Idle,
        Run,
        Done,
    }

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
    #[rhdl(dq_no_prefix)]
    #[fsm(state_field = "state", state_enum = CycleState)]
    pub struct CycleMachine {
        state: dff::DFF<CycleState>,
    }

    impl SynchronousIO for CycleMachine {
        type I = bool;
        type O = bool;
        type Kernel = cycle_kernel;
    }

    #[kernel]
    pub fn cycle_kernel(cr: ClockReset, _i: bool, q: Q) -> (bool, D) {
        let mut d = D::dont_care();
        let next: CycleState = match q.state {
            CycleState::Idle => CycleState::Run,
            CycleState::Run => CycleState::Done,
            CycleState::Done => CycleState::Idle,
        };
        d.state = next;
        if cr.reset.any() {
            d.state = CycleState::Idle;
        }
        let busy = q.state != CycleState::Idle;
        (busy, d)
    }

    #[test]
    fn extract_widget_transitions_recovers_canonical_three_state_cycle() {
        let result = rhdl::core::fsm::extract_widget_transitions::<CycleMachine>()
            .expect("extraction should succeed for a clean three-state cycle kernel");
        assert!(
            result.unanalyzable.is_empty(),
            "no Unanalyzable diagnostics expected, got: {:?}",
            result.unanalyzable
        );
        let mut transitions = result.transitions;
        transitions.sort();
        use rhdl::core::fsm::analysis::Transition;
        assert_eq!(
            transitions,
            vec![
                Transition {
                    source_index: 0,
                    target_index: 1,
                },
                Transition {
                    source_index: 1,
                    target_index: 2,
                },
                Transition {
                    source_index: 2,
                    target_index: 0,
                },
            ]
        );
    }

    #[test]
    fn extract_widget_transitions_strict_returns_sorted_transitions_directly() {
        let transitions = rhdl::core::fsm::extract_widget_transitions_strict::<CycleMachine>()
            .expect("strict extraction should succeed for the canonical kernel");
        use rhdl::core::fsm::analysis::Transition;
        assert_eq!(
            transitions,
            vec![
                Transition {
                    source_index: 0,
                    target_index: 1,
                },
                Transition {
                    source_index: 1,
                    target_index: 2,
                },
                Transition {
                    source_index: 2,
                    target_index: 0,
                },
            ]
        );
    }

    #[test]
    fn write_fsm_diagram_produces_markdown_file_no_manual_list() {
        let filename = "test_cycle_machine_fsm.md";
        write_fsm_diagram::<CycleMachine>(filename)
            .expect("auto-derived diagram emission should succeed");
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("doc")
            .join(filename);
        let content = std::fs::read_to_string(&path).expect("emitted markdown file should exist");
        assert!(content.contains("<svg"));
        assert!(content.contains("Idle"));
        assert!(content.contains("Run"));
        assert!(content.contains("Done"));
        // Cleanup so the test artifact doesn't pollute committed docs.
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn assert_fsm_transitions_match_passes_for_correct_manual_list() {
        use rhdl::core::fsm::analysis::Transition;
        let manual = [
            Transition {
                source_index: 0,
                target_index: 1,
            },
            Transition {
                source_index: 1,
                target_index: 2,
            },
            Transition {
                source_index: 2,
                target_index: 0,
            },
        ];
        assert_fsm_transitions_match::<CycleMachine>(&manual)
            .expect("manual list matches the kernel; drift-check must pass");
    }

    #[test]
    fn assert_fsm_transitions_match_fails_loudly_on_drift() {
        use rhdl::core::fsm::analysis::Transition;
        // Wrong list — claims an Idle→Done edge that the kernel doesn't have.
        let bad = [
            Transition {
                source_index: 0,
                target_index: 2,
            },
            Transition {
                source_index: 1,
                target_index: 2,
            },
            Transition {
                source_index: 2,
                target_index: 0,
            },
        ];
        let err = assert_fsm_transitions_match::<CycleMachine>(&bad).expect_err(
            "drift-check must reject a manual list that doesn't match the derived list",
        );
        let msg = format!("{err}");
        assert!(msg.contains("drift detected"), "unexpected error: {msg}");
    }

    /// End-to-end Phase 2 + Phase 3: derived transitions feed the
    /// diagram renderer with zero author-curated metadata.
    #[test]
    fn extracted_transitions_drive_the_diagram_renderer_directly() {
        use rhdl::core::fsm::diagram::{build_fsm_diagram, render_fsm_svg};
        let transitions =
            rhdl::core::fsm::extract_widget_transitions_strict::<CycleMachine>().unwrap();
        let desc = CycleMachine::fsm_descriptor();
        let diagram = build_fsm_diagram(&desc, &transitions);
        assert_eq!(diagram.widget_name, "CycleMachine");
        assert_eq!(diagram.nodes.len(), 3);
        assert_eq!(diagram.edges.len(), 3);
        let svg = render_fsm_svg(&diagram);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("Idle"));
        assert!(svg.contains("Run"));
        assert!(svg.contains("Done"));
    }
}
