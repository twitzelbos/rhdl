//! Integration test for FSM-transition extraction when the target state
//! cannot be determined.
//!
//! # The defect this fixed
//!
//! `extract_widget_transitions_strict` used to return **success with a
//! silently incomplete transition set** when a kernel wrote its state
//! field from a value the extractor could not trace — in practice, from
//! a kernel input. The walker returned an empty target set, the caller
//! read that as "no state write on this path", and the
//! implicit-self-loop convention turned it into a self-loop edge.
//!
//! The result was a generated diagram asserting the machine could not
//! leave the state it was in, when in truth it could reach *any* state.
//! CLAUDE.md §12 rule 14 makes that diagram load-bearing for FSM-aware
//! review and LLM-assisted refactor, so a silently wrong one is worse
//! than a missing one: a reviewer who trusts it concludes a state
//! machine is a set of isolated self-loops.
//!
//! `ExtractionResult` already carried an `unanalyzable` field and the
//! analysis pass already reported `FsmDiagnosticKind::Unanalyzable`;
//! this path simply did not use them.
//!
//! # What is tested here
//!
//! Three real widgets through the whole pipeline, because the
//! pass-level tests in `rhdl_core::fsm::extraction` work on hand-built
//! op lists and cannot show that a *kernel* written the offending way
//! actually produces the diagnostic.
//!
//! 1. [`from_input`] — the state assigned from an input. Must fail.
//! 2. [`computed`] — the state assigned from an intermediate local that
//!    the arms write literals into. Must succeed, with the full
//!    transition set. This is the guard against the fix being
//!    pessimistic: a mutable local lowers to a `Select` chain, which the
//!    walker unions, and that behaviour must survive.
//! 3. [`literal`] — the canonical literal-per-arm shape. Must succeed.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
/// Three states is the smallest number that distinguishes "can reach
/// any state" from "self-loop only".
pub enum P {
    #[default]
    A,
    B,
    C,
}

/// A target the extractor cannot name: it comes from outside the kernel.
mod from_input {
    use super::P;
    use rhdl::prelude::*;
    use rhdl_fpga::core::dff;

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
    #[rhdl(dq_no_prefix)]
    #[fsm(state_field = "state", state_enum = P, allow_implicit)]
    pub struct FromInput {
        state: dff::DFF<P>,
    }

    impl SynchronousIO for FromInput {
        type I = P;
        type O = P;
        type Kernel = from_input;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn from_input(cr: ClockReset, i: P, q: Q) -> (P, D) {
        let mut d = D::dont_care();
        d.state = q.state;
        match q.state {
            P::A => d.state = i,
            P::B => d.state = i,
            P::C => d.state = i,
        }
        let o = q.state;
        if cr.reset.any() {
            d.state = P::A;
        }
        (o, d)
    }
}

/// A target that *is* traceable, through an intermediate local.
mod computed {
    use super::P;
    use rhdl::prelude::*;
    use rhdl_fpga::core::dff;

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
    #[rhdl(dq_no_prefix)]
    #[fsm(state_field = "state", state_enum = P, allow_implicit)]
    pub struct Computed {
        state: dff::DFF<P>,
    }

    impl SynchronousIO for Computed {
        type I = bool;
        type O = P;
        type Kernel = computed;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn computed(cr: ClockReset, i: bool, q: Q) -> (P, D) {
        let mut d = D::dont_care();
        let mut next = q.state;
        match q.state {
            P::A => {
                if i {
                    next = P::B;
                }
            }
            P::B => {
                if i {
                    next = P::C;
                } else {
                    next = P::A;
                }
            }
            P::C => next = P::A,
        }
        d.state = next;
        let o = q.state;
        if cr.reset.any() {
            d.state = P::A;
        }
        (o, d)
    }
}

/// The canonical shape: a literal target per arm.
mod literal {
    use super::P;
    use rhdl::prelude::*;
    use rhdl_fpga::core::dff;

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
    #[rhdl(dq_no_prefix)]
    #[fsm(state_field = "state", state_enum = P, allow_implicit)]
    pub struct Literal {
        state: dff::DFF<P>,
    }

    impl SynchronousIO for Literal {
        type I = bool;
        type O = P;
        type Kernel = literal;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn literal(cr: ClockReset, i: bool, q: Q) -> (P, D) {
        let mut d = D::dont_care();
        d.state = q.state;
        match q.state {
            P::A => {
                if i {
                    d.state = P::B;
                }
            }
            P::B => d.state = P::C,
            P::C => d.state = P::A,
        }
        let o = q.state;
        if cr.reset.any() {
            d.state = P::A;
        }
        (o, d)
    }
}

fn edges<W>() -> Vec<(usize, usize)>
where
    W: rhdl::core::fsm::FsmWidget + SynchronousIO,
{
    let mut v: Vec<(usize, usize)> = rhdl::core::fsm::extract_widget_transitions_strict::<W>()
        .expect("extraction should succeed")
        .iter()
        .map(|t| (t.source_index, t.target_index))
        .collect();
    v.sort();
    v
}

/// **The regression.** A state written from an input must be reported as
/// unanalysable, not silently turned into a self-loop.
#[test]
fn a_state_written_from_an_input_fails_loudly() {
    let r = rhdl::core::fsm::extract_widget_transitions_strict::<from_input::FromInput>();
    let err = match r {
        Ok(t) => panic!(
            "extraction should have failed; instead it returned {} transitions: {:?}. \
             If these are all self-loops, this is exactly the defect: the diagram would \
             claim the machine never leaves its state.",
            t.len(),
            t.iter()
                .map(|x| (x.source_index, x.target_index))
                .collect::<Vec<_>>()
        ),
        Err(e) => format!("{e}"),
    };
    // The message has to tell a reader what to do about it.
    assert!(
        err.contains("Unanalyzable"),
        "the error should name the diagnostic: {err}"
    );
    assert!(
        err.contains("no definer"),
        "the error should name the cause: {err}"
    );
}

/// The non-strict extractor reports the same thing as data rather than
/// an error, and attributes it to every affected source variant.
#[test]
fn the_non_strict_extractor_reports_one_diagnostic_per_affected_state() {
    let result = rhdl::core::fsm::extract_widget_transitions::<from_input::FromInput>()
        .expect("non-strict extraction returns a result, not an error");
    assert_eq!(
        result.unanalyzable.len(),
        3,
        "all three states write from the input, so all three are unanalysable: {:?}",
        result.unanalyzable
    );
    for (state, _reason) in &result.unanalyzable {
        assert!(
            ["A", "B", "C"].contains(state),
            "unexpected state name {state:?}"
        );
    }
    assert!(
        result.transitions.is_empty(),
        "and no transitions should be invented: {:?}",
        result.transitions
    );
}

/// **The guard against pessimism.** A computed target that *is*
/// traceable must still extract completely — a mutable local lowers to
/// a `Select` chain and the walker unions its branches.
#[test]
fn a_computed_but_traceable_target_still_extracts() {
    // A: holds when `i` is false, goes to B when true.
    // B: C when true, A when false. C: always A.
    assert_eq!(
        edges::<computed::Computed>(),
        vec![(0, 0), (0, 1), (1, 0), (1, 2), (2, 0)]
    );
}

/// And the canonical literal-per-arm shape is untouched.
#[test]
fn the_literal_shape_is_unaffected() {
    assert_eq!(
        edges::<literal::Literal>(),
        vec![(0, 0), (0, 1), (1, 2), (2, 0)]
    );
}

/// The three widgets still build real hardware — the change is to
/// *analysis*, and must not have altered lowering.
#[test]
fn all_three_widgets_still_lower_to_verilog() -> miette::Result<()> {
    let a = from_input::FromInput::default();
    assert!(
        a.descriptor("top".into())?
            .hdl()?
            .modules
            .pretty()
            .contains("module top")
    );
    let b = computed::Computed::default();
    assert!(
        b.descriptor("top".into())?
            .hdl()?
            .modules
            .pretty()
            .contains("module top")
    );
    let c = literal::Literal::default();
    assert!(
        c.descriptor("top".into())?
            .hdl()?
            .modules
            .pretty()
            .contains("module top")
    );
    Ok(())
}

/// And they simulate, so the `from_input` widget is a real state machine
/// rather than a construct that only exists to fail extraction.
#[test]
fn the_from_input_widget_actually_changes_state() {
    let uut = from_input::FromInput::default();
    let stream = [P::B, P::C, P::A, P::C]
        .into_iter()
        .with_reset(1)
        .clock_pos_edge(100);
    let seen: Vec<P> = uut
        .run(stream)
        .synchronous_sample()
        .map(|s| s.output)
        .collect();
    assert!(
        seen.contains(&P::B) && seen.contains(&P::C),
        "the input really does drive the state anywhere, which is why a \
         self-loop-only diagram was wrong: {seen:?}"
    );
    let _ = dff::DFF::<P>::default(); // keep the import honest
}
