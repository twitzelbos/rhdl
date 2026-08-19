use rhdl::core::fsm::analysis::Transition;
use rhdl::core::fsm::diagram::{build_fsm_diagram, render_fsm_svg};
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

/// Refresh-and-check helper for the `#[fsm_doc]` workflow: write the
/// current kernel's FSM diagram to the on-disk file, then verify the
/// result matches.
///
/// # Call this from an example, never from a test
///
/// It was originally documented the other way round — as a `#[test]` so
/// that `cargo test` became the refresh trigger, saving a
/// `cargo run --example` step. That turned out to be wrong twice over:
///
/// - **The check cannot fail on staleness.** Refreshing before verifying
///   means it validates its own write, so a kernel change with a
///   forgotten refresh ships a stale rustdoc diagram and nothing
///   objects. The test that looked like the drift guard was the reason
///   there wasn't one.
/// - **It races any test that reads the same file.** Cargo runs tests in
///   one binary in parallel, so a concurrent reader can observe the file
///   mid-truncation. That was a real, recorded flake.
///
/// It also cuts against the convention set on 2026-08-16
/// (*"`cargo test` no longer rewrites committed traces"*): committed
/// artifacts are refreshed by `cargo run --example …` and only *checked*
/// by tests, so that a dirty working tree means something.
///
/// So use this from a widget's example, alongside the waveform it already
/// writes:
///
/// ```ignore
/// // examples/my_widget.rs
/// rhdl_fpga::doc::refresh_and_check_fsm_diagram::<MyWidget>("MyWidget_fsm.md")?;
/// ```
///
/// and check it from the test with [`assert_fsm_diagram_up_to_date`],
/// which is read-only.
pub fn refresh_and_check_fsm_diagram<W>(filename: &str) -> anyhow::Result<()>
where
    W: rhdl::core::fsm::FsmWidget + SynchronousIO,
{
    write_fsm_diagram::<W>(filename)?;
    assert_fsm_diagram_up_to_date::<W>(filename)
}

/// Drift-check helper for the `#[fsm_doc]` workflow: assert that
/// the on-disk diagram file matches what [`write_fsm_diagram`]
/// would produce *right now* against the current kernel.
///
/// **This is the form tests should use.** It is read-only, so it
/// catches both a kernel change with a forgotten refresh and a
/// renderer-level regression (a change to the SVG layout algorithm
/// fails it even when the FSM itself is unchanged). The check is
/// byte-for-byte.
///
/// Do **not** call [`refresh_and_check_fsm_diagram`] from a test.
/// Refreshing before checking means the check cannot fail on
/// staleness, and writing a committed file from a test races any
/// other test reading it — both of which happened here; see
/// `fsm_doc_committed_diagram_matches_the_kernel`.
///
/// Pairs with the `#[fsm_doc]` attribute macro, which removes the
/// per-widget `#![doc = include_str!(...)]` boilerplate.  See
/// `fsm-architecture.md` Phase 3c/3d for the rationale.
pub fn assert_fsm_diagram_up_to_date<W>(filename: &str) -> anyhow::Result<()>
where
    W: rhdl::core::fsm::FsmWidget + SynchronousIO,
{
    let transitions = rhdl::core::fsm::extract_widget_transitions_strict::<W>()?;
    let desc = W::fsm_descriptor();
    let diagram = rhdl::core::fsm::diagram::build_fsm_diagram(&desc, &transitions);
    let svg = rhdl::core::fsm::diagram::render_fsm_svg(&diagram);
    let expected = format!("\n\n<p>\n{}\n</p>", svg);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("doc")
        .join(filename);
    let actual = std::fs::read_to_string(&path).map_err(|e| {
        anyhow::anyhow!(
            "FSM diagram file {} is missing or unreadable ({}).  Run `cargo run --example <widget> --package rhdl-fpga` to materialise it.",
            path.display(),
            e
        )
    })?;
    if actual != expected {
        anyhow::bail!(
            "FSM diagram file {} is stale.  Re-run `cargo run --example <widget> --package rhdl-fpga` to refresh it.",
            path.display()
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
    #[fsm(state_field = "state", state_enum = CycleState, allow_implicit)]
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

    /// Drift-check on the on-disk SVG file.  Workflow: emit fresh,
    /// then assert; mutate, then assert err.
    #[test]
    fn assert_fsm_diagram_up_to_date_passes_after_fresh_emission() {
        let filename = "test_drift_fresh_fsm.md";
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("doc")
            .join(filename);
        // Fresh emit, then assert it's up-to-date.
        write_fsm_diagram::<CycleMachine>(filename).unwrap();
        assert_fsm_diagram_up_to_date::<CycleMachine>(filename)
            .expect("freshly-emitted file must be up-to-date");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn assert_fsm_diagram_up_to_date_fails_on_stale_file() {
        let filename = "test_drift_stale_fsm.md";
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("doc")
            .join(filename);
        // Write a stale file with bogus content.
        std::fs::create_dir_all(path.parent().unwrap()).ok();
        std::fs::write(&path, "<not the right svg>").unwrap();
        let err = assert_fsm_diagram_up_to_date::<CycleMachine>(filename)
            .expect_err("stale file must be rejected");
        let msg = format!("{err}");
        assert!(msg.contains("stale"), "unexpected error: {msg}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn assert_fsm_diagram_up_to_date_fails_on_missing_file() {
        let filename = "test_drift_missing_fsm.md";
        // Make sure the file does NOT exist.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("doc")
            .join(filename);
        std::fs::remove_file(&path).ok();
        let err = assert_fsm_diagram_up_to_date::<CycleMachine>(filename)
            .expect_err("missing file must be rejected");
        let msg = format!("{err}");
        assert!(msg.contains("missing"), "unexpected error: {msg}");
    }

    /// The committed `#[fsm_doc]` diagram matches the current kernel.
    ///
    /// **Read-only, deliberately.** This test used to call
    /// [`refresh_and_check_fsm_diagram`], rewriting the committed file
    /// from the kernel and then verifying its own write. Two things were
    /// wrong with that, and they compounded:
    ///
    /// 1. **It could not detect drift.** Refreshing before checking means
    ///    the check always passes, so a kernel change with a forgotten
    ///    refresh shipped a stale rustdoc diagram silently. The test that
    ///    looked like the drift guard was the reason there wasn't one.
    /// 2. **It raced with the reader.** A sibling test read the same file
    ///    while this one truncated and rewrote it. Same binary, run in
    ///    parallel by cargo, so the reader intermittently saw a partial
    ///    file — the flake recorded in the CHANGELOG.
    ///
    /// It also violated the convention established on 2026-08-16
    /// (*"`cargo test` no longer rewrites committed traces"*): every other
    /// artifact in this crate is refreshed by `cargo run --example …` and
    /// only *checked* by tests. This one had been left out.
    ///
    /// So the two tests are merged into this one, which reads and compares
    /// and never writes. Refresh with
    /// `cargo run --example fsm_doc_demo --package rhdl-fpga`.
    #[test]
    fn fsm_doc_committed_diagram_matches_the_kernel() {
        use crate::doc::demo::AutoDocMachine;
        assert_fsm_diagram_up_to_date::<AutoDocMachine>("AutoDocMachine_fsm.md").expect(
            "the committed FSM diagram is stale or the renderer regressed; \
             refresh with `cargo run --example fsm_doc_demo --package rhdl-fpga`",
        );
    }

    // -----------------------------------------------------------
    // Adversarial integration tests for the side-effect-form
    // FSM extractor (PR `feat/fsm-extractor-side-effects`).  Each
    // sub-mod defines a real `Synchronous` + `FsmWidget` widget
    // whose kernel exercises a distinct kernel-language idiom.
    // The test compiles the kernel through Stage 1, runs the
    // canonical transition extractor, and asserts the recovered
    // graph against a hand-derived expected set.
    //
    // These exist to guarantee that EVERY kernel idiom an author
    // can plausibly use to update `d.state` is either extracted
    // correctly or surfaces a precise Unanalyzable diagnostic.
    // Combined with the synthetic-RHIF unit tests in
    // `rhdl_core::fsm::extraction::tests`, this gives end-to-end
    // coverage of the extractor.
    // -----------------------------------------------------------

    use rhdl::core::fsm::analysis::Transition;

    /// Helper to drive the extractor against a widget and return its
    /// sorted derived transitions.  Asserts no Unanalyzable
    /// diagnostics — a positive test fails loudly if the kernel
    /// shape isn't fully recognised.
    fn extract_or_fail<W>() -> Vec<Transition>
    where
        W: rhdl::core::fsm::FsmWidget + SynchronousIO,
    {
        let result = rhdl::core::fsm::extract_widget_transitions::<W>().expect("compile + extract");
        assert!(
            result.unanalyzable.is_empty(),
            "Unanalyzable diagnostics for {}: {:?}",
            std::any::type_name::<W>(),
            result.unanalyzable
        );
        let mut t = result.transitions;
        t.sort();
        t
    }

    // ---- Adversarial widget #1: side-effect with conditional + default ----
    mod adv_sideeffect_conditional {
        use super::*;
        #[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
        pub enum S {
            #[default]
            A,
            B,
            C,
        }
        #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
        #[rhdl(dq_no_prefix)]
        #[fsm(state_field = "state", state_enum = S, allow_implicit)]
        pub struct W {
            state: dff::DFF<S>,
        }
        impl SynchronousIO for W {
            type I = bool;
            type O = bool;
            type Kernel = k;
        }
        #[kernel]
        pub fn k(cr: ClockReset, i: bool, q: Q) -> (bool, D) {
            let mut d = D::dont_care();
            d.state = q.state;
            match q.state {
                S::A => {
                    if i {
                        d.state = S::B;
                    }
                }
                S::B => {
                    d.state = S::C;
                }
                S::C => {
                    d.state = S::A;
                }
            }
            if cr.reset.any() {
                d.state = S::A;
            }
            (false, d)
        }
        #[test]
        fn extracts() {
            let t = extract_or_fail::<W>();
            // A→A (self-loop via default), A→B (taken), B→C, C→A.
            assert_eq!(
                t,
                vec![
                    Transition {
                        source_index: 0,
                        target_index: 0
                    },
                    Transition {
                        source_index: 0,
                        target_index: 1
                    },
                    Transition {
                        source_index: 1,
                        target_index: 2
                    },
                    Transition {
                        source_index: 2,
                        target_index: 0
                    },
                ]
            );
        }

        /// Property-based check: simulator-observed ⊆ extractor.
        /// Per `fsm-architecture.md` §5.4.2 #2.
        #[test]
        fn property_simulator_observed_is_subset_of_extractor_output() {
            use rhdl::core::fsm::analysis::Transition;
            use std::collections::BTreeSet;

            let extractor =
                rhdl::core::fsm::extract_widget_transitions::<W>().expect("compile + extract");
            assert!(extractor.unanalyzable.is_empty());
            let extracted: BTreeSet<_> = extractor.transitions.iter().collect();

            let mut observed: BTreeSet<Transition> = BTreeSet::new();
            let cr = clock_reset(clock(false), reset(false));

            for (src_idx, src_state) in [(0usize, S::A), (1, S::B), (2, S::C)] {
                for &i_val in &[false, true] {
                    let q = Q { state: src_state };
                    let (_o, d) = k(cr, i_val, q);
                    let target_idx = match d.state {
                        S::A => 0,
                        S::B => 1,
                        S::C => 2,
                    };
                    observed.insert(Transition {
                        source_index: src_idx,
                        target_index: target_idx,
                    });
                }
            }

            for obs in &observed {
                assert!(
                    extracted.contains(obs),
                    "Simulator observed {obs:?} but extractor missed it"
                );
            }
            assert!(!observed.is_empty());
        }
    }

    // ---- Adversarial widget #2: nested if-else inside one arm ----
    mod adv_nested_if_else {
        use super::*;
        #[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
        pub enum S {
            #[default]
            A,
            B,
            C,
            D2,
        }
        #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
        #[rhdl(dq_no_prefix)]
        #[fsm(state_field = "state", state_enum = S, allow_implicit)]
        pub struct W {
            state: dff::DFF<S>,
        }
        impl SynchronousIO for W {
            type I = (bool, bool);
            type O = bool;
            type Kernel = k;
        }
        #[kernel]
        pub fn k(cr: ClockReset, i: (bool, bool), q: Q) -> (bool, D) {
            let mut d = D::dont_care();
            d.state = q.state;
            let (c1, c2) = i;
            match q.state {
                S::A => {
                    if c1 {
                        d.state = S::B;
                    } else if c2 {
                        d.state = S::C;
                    } else {
                        d.state = S::D2;
                    }
                }
                S::B => d.state = S::A,
                S::C => d.state = S::A,
                S::D2 => d.state = S::A,
            }
            if cr.reset.any() {
                d.state = S::A;
            }
            (false, d)
        }
        #[test]
        fn extracts() {
            let t = extract_or_fail::<W>();
            // A → {B, C, D2}; B,C,D2 → A.
            assert_eq!(
                t,
                vec![
                    Transition {
                        source_index: 0,
                        target_index: 1
                    }, // A → B
                    Transition {
                        source_index: 0,
                        target_index: 2
                    }, // A → C
                    Transition {
                        source_index: 0,
                        target_index: 3
                    }, // A → D2
                    Transition {
                        source_index: 1,
                        target_index: 0
                    }, // B → A
                    Transition {
                        source_index: 2,
                        target_index: 0
                    }, // C → A
                    Transition {
                        source_index: 3,
                        target_index: 0
                    }, // D2 → A
                ]
            );
        }
    }

    // ---- Adversarial widget #3: let-binding form (existing pattern) ----
    mod adv_let_binding {
        use super::*;
        #[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
        pub enum S {
            #[default]
            A,
            B,
            C,
        }
        #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
        #[rhdl(dq_no_prefix)]
        #[fsm(state_field = "state", state_enum = S, allow_implicit)]
        pub struct W {
            state: dff::DFF<S>,
        }
        impl SynchronousIO for W {
            type I = bool;
            type O = bool;
            type Kernel = k;
        }
        #[kernel]
        pub fn k(cr: ClockReset, _i: bool, q: Q) -> (bool, D) {
            let mut d = D::dont_care();
            let next = match q.state {
                S::A => S::B,
                S::B => S::C,
                S::C => S::A,
            };
            d.state = next;
            if cr.reset.any() {
                d.state = S::A;
            }
            (false, d)
        }
        #[test]
        fn extracts() {
            let t = extract_or_fail::<W>();
            assert_eq!(
                t,
                vec![
                    Transition {
                        source_index: 0,
                        target_index: 1
                    },
                    Transition {
                        source_index: 1,
                        target_index: 2
                    },
                    Transition {
                        source_index: 2,
                        target_index: 0
                    },
                ]
            );
        }
    }

    // ---- Adversarial widget #4: computed-then-assigned (mixed form) ----
    mod adv_computed_then_assigned {
        use super::*;
        #[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
        pub enum S {
            #[default]
            A,
            B,
            C,
        }
        #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
        #[rhdl(dq_no_prefix)]
        #[fsm(state_field = "state", state_enum = S, allow_implicit)]
        pub struct W {
            state: dff::DFF<S>,
        }
        impl SynchronousIO for W {
            type I = bool;
            type O = bool;
            type Kernel = k;
        }
        #[kernel]
        pub fn k(cr: ClockReset, i: bool, q: Q) -> (bool, D) {
            let mut d = D::dont_care();
            d.state = q.state;
            match q.state {
                S::A => {
                    let next = if i { S::B } else { S::C };
                    d.state = next;
                }
                S::B => d.state = S::A,
                S::C => d.state = S::A,
            }
            if cr.reset.any() {
                d.state = S::A;
            }
            (false, d)
        }
        #[test]
        fn extracts() {
            let t = extract_or_fail::<W>();
            // A → {B, C} via let-bound select inside arm. B,C → A.
            assert_eq!(
                t,
                vec![
                    Transition {
                        source_index: 0,
                        target_index: 1
                    },
                    Transition {
                        source_index: 0,
                        target_index: 2
                    },
                    Transition {
                        source_index: 1,
                        target_index: 0
                    },
                    Transition {
                        source_index: 2,
                        target_index: 0
                    },
                ]
            );
        }
    }

    // ---- Adversarial widget #5: arm-with-no-state-assignment uses default ----
    mod adv_arm_with_no_assignment {
        use super::*;
        #[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
        pub enum S {
            #[default]
            A,
            B,
            C,
        }
        #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
        #[rhdl(dq_no_prefix)]
        #[fsm(state_field = "state", state_enum = S, allow_implicit)]
        pub struct W {
            state: dff::DFF<S>,
        }
        impl SynchronousIO for W {
            type I = bool;
            type O = bool;
            type Kernel = k;
        }
        #[kernel]
        pub fn k(cr: ClockReset, _i: bool, q: Q) -> (bool, D) {
            let mut d = D::dont_care();
            d.state = q.state;
            match q.state {
                S::A => d.state = S::B,
                S::B => d.state = S::C,
                S::C => {} // empty arm — state stays at q.state (self-loop)
            }
            if cr.reset.any() {
                d.state = S::A;
            }
            (false, d)
        }
        #[test]
        fn extracts() {
            let t = extract_or_fail::<W>();
            assert_eq!(
                t,
                vec![
                    Transition {
                        source_index: 0,
                        target_index: 1
                    }, // A → B
                    Transition {
                        source_index: 1,
                        target_index: 2
                    }, // B → C
                    Transition {
                        source_index: 2,
                        target_index: 2
                    }, // C → C (empty arm preserves)
                ]
            );
        }
    }

    // ---- Adversarial widget #6: multi-arm with mixed conditional + unconditional ----
    mod adv_mixed_arms {
        use super::*;
        #[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
        pub enum S {
            #[default]
            Idle,
            Active,
            Cooldown,
            Error,
        }
        #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
        #[rhdl(dq_no_prefix)]
        #[fsm(state_field = "state", state_enum = S, allow_implicit)]
        pub struct W {
            state: dff::DFF<S>,
        }
        impl SynchronousIO for W {
            type I = (bool, bool);
            type O = bool;
            type Kernel = k;
        }
        #[kernel]
        pub fn k(cr: ClockReset, i: (bool, bool), q: Q) -> (bool, D) {
            let mut d = D::dont_care();
            d.state = q.state;
            let (start, fault) = i;
            match q.state {
                S::Idle => {
                    if start {
                        d.state = S::Active;
                    }
                }
                S::Active => {
                    if fault {
                        d.state = S::Error;
                    } else {
                        d.state = S::Cooldown;
                    }
                }
                S::Cooldown => d.state = S::Idle,
                S::Error => {} // stuck
            }
            if cr.reset.any() {
                d.state = S::Idle;
            }
            (false, d)
        }
        #[test]
        fn extracts() {
            let t = extract_or_fail::<W>();
            assert_eq!(
                t,
                vec![
                    Transition {
                        source_index: 0,
                        target_index: 0
                    }, // Idle self-loop (start=false)
                    Transition {
                        source_index: 0,
                        target_index: 1
                    }, // Idle → Active
                    Transition {
                        source_index: 1,
                        target_index: 2
                    }, // Active → Cooldown
                    Transition {
                        source_index: 1,
                        target_index: 3
                    }, // Active → Error
                    Transition {
                        source_index: 2,
                        target_index: 0
                    }, // Cooldown → Idle
                    Transition {
                        source_index: 3,
                        target_index: 3
                    }, // Error self-loop
                ]
            );
        }
    }

    // ---- Adversarial widget #7: bare-match output (let-binding) with self-loop branch ----
    mod adv_let_binding_with_self_loop_branch {
        use super::*;
        #[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
        pub enum S {
            #[default]
            A,
            B,
        }
        #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
        #[rhdl(dq_no_prefix)]
        #[fsm(state_field = "state", state_enum = S, allow_implicit)]
        pub struct W {
            state: dff::DFF<S>,
        }
        impl SynchronousIO for W {
            type I = bool;
            type O = bool;
            type Kernel = k;
        }
        #[kernel]
        pub fn k(cr: ClockReset, i: bool, q: Q) -> (bool, D) {
            let mut d = D::dont_care();
            let next = match q.state {
                S::A => {
                    if i {
                        S::B
                    } else {
                        S::A
                    }
                }
                S::B => S::A,
            };
            d.state = next;
            if cr.reset.any() {
                d.state = S::A;
            }
            (false, d)
        }
        #[test]
        fn extracts() {
            let t = extract_or_fail::<W>();
            assert_eq!(
                t,
                vec![
                    Transition {
                        source_index: 0,
                        target_index: 0
                    }, // A → A (else)
                    Transition {
                        source_index: 0,
                        target_index: 1
                    }, // A → B (then)
                    Transition {
                        source_index: 1,
                        target_index: 0
                    }, // B → A
                ]
            );
        }
    }

    // ---- Adversarial widget #8: can_master-shaped — guarded ----
    // ---- transition + else-branch writing a different field ----
    //
    // The motivating real-world widget shape (per
    // crates/rhdl-fpga/src/core/can_master.rs lines 354 + 495):
    // kernel-top default `d.field = q.field` + `match q.field {
    // X => if guard { d.field = NextX; d.bit_idx = 0 } else {
    // d.bit_idx = bit_idx + 1 } }`.  The else-branch writes ONLY
    // d.bit_idx, never d.field — but the canonical RHDL pattern
    // (kernel-top default) means the state holds.  Pre-fix, the
    // extractor flagged this arm as Unanalyzable with diagnostic
    // "neither value-form nor d-struct-form walker found a state
    // assignment in this arm".  Post-fix, the implicit-self-loop
    // semantics correctly emit a self-loop on the source variant.
    mod adv_can_master_guarded_else_writes_other_field {
        use super::*;
        #[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
        pub enum S {
            #[default]
            Sof,
            Id,
            Eof,
        }
        #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
        #[rhdl(dq_no_prefix)]
        #[fsm(state_field = "state", state_enum = S, allow_implicit)]
        pub struct W {
            state: dff::DFF<S>,
            bit_idx: dff::DFF<rhdl::bits::Bits<4>>,
        }
        impl SynchronousIO for W {
            type I = bool;
            type O = bool;
            type Kernel = k;
        }
        #[kernel]
        pub fn k(cr: ClockReset, _i: bool, q: Q) -> (bool, D) {
            let mut d = D::dont_care();
            // Canonical RHDL kernel-top defaults — the source of
            // the implicit-self-loop semantics.
            d.state = q.state;
            d.bit_idx = q.bit_idx;
            let max: rhdl::bits::Bits<4> = rhdl::bits::bits::<4>(10);
            let one: rhdl::bits::Bits<4> = rhdl::bits::bits::<4>(1);
            let zero: rhdl::bits::Bits<4> = rhdl::bits::bits::<4>(0);
            match q.state {
                S::Sof => {
                    // Unconditional transition: Sof → Id.
                    d.state = S::Id;
                    d.bit_idx = zero;
                }
                S::Id => {
                    // The motivating shape: guarded transition,
                    // else-branch writes only the bit counter.
                    if q.bit_idx == max {
                        d.state = S::Eof;
                        d.bit_idx = zero;
                    } else {
                        d.bit_idx = q.bit_idx + one;
                    }
                }
                S::Eof => {
                    // Same shape — wraps to Sof at end-of-frame.
                    if q.bit_idx == max {
                        d.state = S::Sof;
                        d.bit_idx = zero;
                    } else {
                        d.bit_idx = q.bit_idx + one;
                    }
                }
            }
            if cr.reset.any() {
                d.state = S::Sof;
                d.bit_idx = zero;
            }
            (false, d)
        }
        #[test]
        fn extracts() {
            let t = extract_or_fail::<W>();
            // Sof → Id (unconditional, no self-loop because no
            // implicit branch).  Id → Eof (then) + Id → Id
            // (implicit self-loop on else).  Eof → Sof + Eof →
            // Eof (same shape).
            assert_eq!(
                t,
                vec![
                    Transition {
                        source_index: 0,
                        target_index: 1
                    }, // Sof → Id
                    Transition {
                        source_index: 1,
                        target_index: 1
                    }, // Id → Id (else)
                    Transition {
                        source_index: 1,
                        target_index: 2
                    }, // Id → Eof (then)
                    Transition {
                        source_index: 2,
                        target_index: 0
                    }, // Eof → Sof (then)
                    Transition {
                        source_index: 2,
                        target_index: 2
                    }, // Eof → Eof (else)
                ]
            );
        }

        /// Property-based check #1 against the canonical 3-state
        /// can_master-shape kernel: enumerate every (source variant,
        /// bit_idx ∈ {0, max}) input combination, call the kernel
        /// directly, observe d.state, build the simulator-observed
        /// set of transitions, assert it is a subset of the
        /// extractor's output.
        ///
        /// This validates **soundness** (no false negatives) — every
        /// transition the simulator can produce IS in the extractor's
        /// graph.  Per `fsm-architecture.md` §5.4.2 #2 (NECESSARY
        /// follow-up converted to in-PR work).
        ///
        /// The complementary direction (every extractor edge is
        /// simulator-reachable) is generally NOT enforceable for the
        /// canonical kernel pattern: the implicit self-loops emitted
        /// by the kernel-top default are sound in the I/O sense but
        /// may require inputs the limited test space doesn't cover.
        /// Documented here, not asserted.
        #[test]
        fn property_simulator_observed_is_subset_of_extractor_output() {
            use rhdl::core::fsm::analysis::Transition;
            use std::collections::BTreeSet;

            let extractor =
                rhdl::core::fsm::extract_widget_transitions::<W>().expect("compile + extract");
            assert!(extractor.unanalyzable.is_empty());
            let extracted: BTreeSet<_> = extractor.transitions.iter().collect();

            // Enumerate inputs: bit_idx is bool here (the widget's
            // input I = bool), so 2 input values.  3 source variants.
            let mut observed: BTreeSet<Transition> = BTreeSet::new();
            let cr = clock_reset(clock(false), reset(false));

            let source_states = [(0usize, S::Sof), (1, S::Id), (2, S::Eof)];
            for (src_idx, src_state) in source_states {
                for &i_val in &[false, true] {
                    let q = Q {
                        state: src_state,
                        bit_idx: rhdl::bits::bits::<4>(if i_val { 10 } else { 0 }),
                    };
                    let (_o, d) = k(cr, false, q);
                    let target_idx = match d.state {
                        S::Sof => 0,
                        S::Id => 1,
                        S::Eof => 2,
                    };
                    observed.insert(Transition {
                        source_index: src_idx,
                        target_index: target_idx,
                    });
                }
            }

            // Soundness: every simulator-observed transition must
            // be in the extractor's output.  Failure here = the
            // extractor missed a real edge.
            for obs in &observed {
                assert!(
                    extracted.contains(obs),
                    "Simulator observed {obs:?} but extractor missed it. \n\
                     Observed: {observed:?}\nExtracted: {extracted:?}"
                );
            }
            assert!(
                !observed.is_empty(),
                "Property test produced no observations — sanity check failed"
            );
        }
    }

    // ---- Adversarial widget #9: nested-conditional implicit ----
    // ---- self-loops (multiple guards inside one arm) ----
    //
    // Stresses the Select-union behaviour: a single arm with
    // nested if/else where TWO branches omit the d.state write.
    // The walker must visit each empty Select branch and emit a
    // self-loop contribution at each union point.
    mod adv_nested_conditional_implicit_self_loops {
        use super::*;
        #[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
        pub enum S {
            #[default]
            A,
            B,
        }
        #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
        #[rhdl(dq_no_prefix)]
        #[fsm(state_field = "state", state_enum = S, allow_implicit)]
        pub struct W {
            state: dff::DFF<S>,
            ctr: dff::DFF<rhdl::bits::Bits<4>>,
        }
        impl SynchronousIO for W {
            type I = (bool, bool);
            type O = bool;
            type Kernel = k;
        }
        #[kernel]
        pub fn k(cr: ClockReset, i: (bool, bool), q: Q) -> (bool, D) {
            let mut d = D::dont_care();
            d.state = q.state;
            d.ctr = q.ctr;
            let zero: rhdl::bits::Bits<4> = rhdl::bits::bits::<4>(0);
            let one: rhdl::bits::Bits<4> = rhdl::bits::bits::<4>(1);
            match q.state {
                S::A => {
                    if i.0 {
                        if i.1 {
                            d.state = S::B;
                        } else {
                            // Nested else: no d.state write.
                            d.ctr = q.ctr + one;
                        }
                    } else {
                        // Outer else: also no d.state write.
                        d.ctr = zero;
                    }
                }
                S::B => {
                    d.state = S::A;
                }
            }
            if cr.reset.any() {
                d.state = S::A;
                d.ctr = zero;
            }
            (false, d)
        }
        #[test]
        fn extracts() {
            let t = extract_or_fail::<W>();
            // A → B (the inner-then branch), A → A (TWO implicit
            // paths union to one self-loop), B → A.
            assert_eq!(
                t,
                vec![
                    Transition {
                        source_index: 0,
                        target_index: 0
                    }, // A → A (implicit, deduped)
                    Transition {
                        source_index: 0,
                        target_index: 1
                    }, // A → B
                    Transition {
                        source_index: 1,
                        target_index: 0
                    }, // B → A
                ]
            );
        }
    }
}

/// Phase-3c demo widget — exercises the `#[fsm_doc]` attribute
/// macro end-to-end at compile time.  Lives in non-test code
/// because the attribute's `include_str!` expansion runs at every
/// build, requiring `doc/AutoDocMachine_fsm.md` to exist on disk
/// regardless of whether tests are being built.
///
/// Materialise the included file by running:
/// `cargo run --example fsm_doc_demo --package rhdl-fpga`.
pub mod demo {
    use crate::core::dff;
    use rhdl::prelude::*;

    /// Three-state cycle FSM (Idle → Run → Done → Idle).  Identical
    /// to the `CycleState` used by the in-test fixtures, but with
    /// its own enum identity so the `#[derive(Fsm)]` doesn't clash
    /// with the test-only one.
    #[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
    pub enum DemoState {
        #[default]
        Idle,
        Run,
        Done,
    }

    /// FSM-widget demo struct.  The `#[fsm_doc]` attribute emits
    /// `#[doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"),
    /// "/doc/AutoDocMachine_fsm.md"))]` on this struct, so the
    /// rustdoc page for `AutoDocMachine` shows the auto-derived
    /// state diagram with no per-widget `#![doc = include_str!]`
    /// boilerplate in the source.
    #[fsm_doc]
    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
    #[rhdl(dq_no_prefix)]
    #[fsm(state_field = "state", state_enum = DemoState, allow_implicit)]
    pub struct AutoDocMachine {
        state: dff::DFF<DemoState>,
    }

    impl SynchronousIO for AutoDocMachine {
        type I = bool;
        type O = bool;
        type Kernel = auto_doc_kernel;
    }

    #[kernel]
    /// Three-state cycle kernel for [AutoDocMachine].
    pub fn auto_doc_kernel(cr: ClockReset, _i: bool, q: Q) -> (bool, D) {
        let mut d = D::dont_care();
        let next: DemoState = match q.state {
            DemoState::Idle => DemoState::Run,
            DemoState::Run => DemoState::Done,
            DemoState::Done => DemoState::Idle,
        };
        d.state = next;
        if cr.reset.any() {
            d.state = DemoState::Idle;
        }
        let busy = q.state != DemoState::Idle;
        (busy, d)
    }
}

/// Render the FSM diagram for a `#[derive(FsmWidget)]`-tagged widget
/// as a self-contained inline-SVG markdown fragment.  Required by
/// every FSM-tagged widget per CLAUDE.md §12 rule 14.
///
/// The caller passes a manually-curated transition list — until
/// the RHIF-extraction pass is wired into the rustdoc emission
/// pipeline, the widget author records the transitions in source
/// alongside the kernel.
pub fn render_fsm_diagram_markdown<W: FsmWidget>(transitions: &[Transition]) -> String {
    let desc = W::fsm_descriptor();
    let diagram = build_fsm_diagram(&desc, transitions);
    let svg = render_fsm_svg(&diagram);
    format!("\n\n<p>\n{svg}\n</p>\n")
}

/// Same as [`render_fsm_diagram_markdown`], but writes the result
/// directly to `doc/<filename>`.
///
/// Convention: widgets named `<name>` write their FSM diagram to
/// `doc/<name>_fsm.md`, and include it in their rustdoc with
/// `#![doc = include_str!("../../doc/<name>_fsm.md")]`.
pub fn write_fsm_diagram_as_markdown<W: FsmWidget>(
    transitions: &[Transition],
    filename: &str,
) -> std::io::Result<()> {
    let md = render_fsm_diagram_markdown::<W>(transitions);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("doc")
        .join(filename);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, md)
}

/// A tiny deterministic pseudo-random source for examples.
///
/// # Why examples must not use `rand`
///
/// Every example in this crate writes a committed artifact — the
/// `doc/<name>.md` trace that its widget's rustdoc embeds via
/// `include_str!`. And because that rustdoc includes the example inside
/// a fenced code block, **the example also runs as a doctest**. So
/// `cargo test` executes every example and rewrites every trace.
///
/// If an example draws from `rand::random`, its trace differs on every
/// run: `cargo test` mutates the working tree, `git status` is never
/// clean after a test run, and the committed artifact is noise rather
/// than a reviewable record of behaviour. It also violates the
/// determinism requirement in CLAUDE.md §12 rule 10.
///
/// This gives examples the *irregular* stimulus they want — bursty
/// sources, uneven backpressure — while staying reproducible. It is a
/// plain xorshift; it is not, and does not need to be, a good RNG.
#[derive(Debug, Clone)]
pub struct DetRng(u32);

impl DetRng {
    /// Create a generator from a seed. Any non-zero seed will do;
    /// different seeds give different-looking traces.
    #[must_use]
    pub fn new(seed: u32) -> Self {
        // Zero is a fixed point of xorshift, so fold it away.
        Self(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    /// Next raw value.
    pub fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    /// True roughly `percent` of the time.
    ///
    /// The deterministic stand-in for `rand::random::<f64>() > p`.
    pub fn chance(&mut self, percent: u32) -> bool {
        self.next_u32() % 100 < percent.min(100)
    }

    /// A value in `0..n`.
    pub fn below(&mut self, n: u128) -> u128 {
        if n == 0 {
            0
        } else {
            u128::from(self.next_u32()) % n
        }
    }
}

#[cfg(test)]
mod det_rng_tests {
    use super::DetRng;

    /// The whole point: same seed, same sequence, every run.
    #[test]
    fn is_reproducible() {
        let a: Vec<u32> = (0..8).map(|_| DetRng::new(7).next_u32()).collect();
        let mut r = DetRng::new(7);
        let b: Vec<u32> = (0..8).map(|_| r.next_u32()).collect();
        assert_eq!(a[0], b[0], "same seed must give the same first value");
        let mut r2 = DetRng::new(7);
        let c: Vec<u32> = (0..8).map(|_| r2.next_u32()).collect();
        assert_eq!(b, c, "the whole sequence must repeat");
    }

    /// `chance` must actually vary, and roughly honour its odds —
    /// a constant would silently remove the irregularity examples want.
    #[test]
    fn chance_varies_and_is_roughly_calibrated() {
        let mut r = DetRng::new(1);
        let hits = (0..1000).filter(|_| r.chance(30)).count();
        assert!(hits > 150 && hits < 450, "expected ~30%, got {hits}/1000");
        let mut r = DetRng::new(1);
        assert!(!(0..50).all(|_| r.chance(50)), "must not be constant true");
        let mut r = DetRng::new(1);
        assert!((0..50).any(|_| r.chance(50)), "must not be constant false");
    }

    /// A zero seed must not collapse the generator (xorshift's fixed point).
    #[test]
    fn zero_seed_still_generates() {
        let mut r = DetRng::new(0);
        let v: Vec<u32> = (0..4).map(|_| r.next_u32()).collect();
        assert!(v.iter().any(|x| *x != 0), "zero seed must not stay zero");
    }

    #[test]
    fn below_stays_in_range() {
        let mut r = DetRng::new(42);
        assert!((0..200).all(|_| r.below(16) < 16));
        assert_eq!(r.below(0), 0, "below(0) must not divide by zero");
    }
}
