//! The queue-pair state machine — what makes "not ready to send" a
//! property of the hardware.
//!
//! A queue pair is not usable the moment it exists. The IBTA puts it
//! through a sequence of states as the two ends exchange parameters,
//! and a requester may only transmit once it reaches **RTS** (Ready To
//! Send). Until this widget existed that rule lived in the caller's
//! discipline: [`crate::tx::RoceWriteTx`] would happily build frames
//! for a connection the peer had never heard of.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +----+QpStateMachine+----+
     |                        |
?Req |                        | QpState
+--->| request          state +--->
     |                        |
bool |                     can+---> bool
+--->| error              send|
     |                        |
     |                draining+---> bool
     |                        |
     |                rejected+---> bool
     +------------------------+
")]
//!
//!# The states, and the ones left out
//!
//! [`QpState`] carries RESET, INIT, RTR, RTS, SQD and ERR. **SQE is
//! deliberately absent.** It is entered when a work queue element
//! completes with an error on an unreliable queue pair, which requires
//! per-element completion status — and that arrives with the work
//! completion machinery in a later phase. Adding the variant now would
//! produce a state nothing can enter, which is worse than not having
//! it: it would appear in the generated diagram as an unreachable node
//! and imply a transition path that does not exist.
//!
//!# Legal transitions
//!
//! The arms below are written as literal targets rather than computed
//! ones, which is both what a real state machine looks like and what
//! the `FsmWidget` extractor requires — it walks RHIF for writes to the
//! state field, and `d.state = <a runtime value>` tells it nothing.
//!
//! They are also **three nested flat matches** rather than the more
//! readable `Some(QpState::Init) => …`. A variant pattern nested inside
//! `Some(…)` is rejected with `UnsupportedPatternInBindPattern`, so the
//! `Option` is unwrapped by its own match first. The `q.state` match
//! stays outermost so the extractor still finds the state field at the
//! top of the tree.
//!
//! ```text
//!   RESET --> INIT --> RTR --> RTS <--> SQD
//!     ^                         |        |
//!     |                         v        v
//!     +----------------------- ERR <-----+        (and any state -> RESET)
//! ```
//!
//! A request for an illegal transition is **refused, not obeyed**: the
//! state holds and `rejected` is asserted for that cycle. The
//! alternative — entering ERR on an illegal request — would make a
//! software bug in the control plane indistinguishable from a wire
//! error, and the two want different responses.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/qp_state.rs")]
//!```
//!
//! The state machine this implements:
#![doc = include_str!("../../doc/qp_state_fsm.md")]
//!
//! And the trace below demonstrates the result.
#![doc = include_str!("../../doc/qp_state.md")]

use rhdl::prelude::*;
use rhdl_fpga::core::dff;

/// Queue-pair state, per IBTA §10.3.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum QpState {
    /// Freshly created or torn down. No context is valid.
    #[default]
    Reset,
    /// Context partially written; cannot send or receive.
    Init,
    /// Ready To Receive — the responder's half is configured.
    Rtr,
    /// Ready To Send. **The only state in which a requester may
    /// transmit.**
    Rts,
    /// Send Queue Drain — finish what is outstanding, accept nothing
    /// new. Used to quiesce a connection without tearing it down.
    Sqd,
    /// Error. Requires a transition through [`QpState::Reset`] to
    /// recover.
    Err,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to the [`QpStateMachine`].
pub struct In {
    /// A requested state, from the control plane — the verbs layer
    /// today, the connection manager once that exists. `None` means no
    /// request this cycle.
    pub request: Option<QpState>,
    /// The datapath detected an error. Forces [`QpState::Err`] and
    /// overrides any request on the same cycle, because a wire error is
    /// not something the control plane gets to disagree with.
    pub error: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs of the [`QpStateMachine`].
pub struct Out {
    /// The current state.
    pub state: QpState,
    /// The requester may transmit. True in [`QpState::Rts`] only.
    pub can_send: bool,
    /// Finish outstanding work but accept no new requests. True in
    /// [`QpState::Sqd`].
    pub draining: bool,
    /// The request on this cycle was refused as an illegal transition.
    /// The state did not change.
    pub rejected: bool,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = QpState, allow_implicit)]
/// The IBTA queue-pair state machine.
pub struct QpStateMachine {
    state: dff::DFF<QpState>,
}

impl SynchronousIO for QpStateMachine {
    type I = In;
    type O = Out;
    type Kernel = qp_state_machine;
}

#[kernel]
/// Kernel for [`QpStateMachine`].
pub fn qp_state_machine(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    let mut o = Out::dont_care();
    d.state = q.state;
    let mut rejected = false;

    // Every arm names a literal target state. Verbose, and the reason
    // is in the module docs: a computed target is invisible to the FSM
    // extractor, so the generated diagram would be empty.
    match q.state {
        QpState::Reset => match i.request {
            Some(req) => match req {
                // A request for the current state is a no-op, not an error.
                QpState::Reset => {}
                QpState::Init => d.state = QpState::Init,
                QpState::Rtr => rejected = true,
                QpState::Rts => rejected = true,
                QpState::Sqd => rejected = true,
                QpState::Err => d.state = QpState::Err,
            },
            None => {}
        },
        QpState::Init => match i.request {
            Some(req) => match req {
                QpState::Reset => d.state = QpState::Reset,
                // A request for the current state is a no-op, not an error.
                QpState::Init => {}
                QpState::Rtr => d.state = QpState::Rtr,
                QpState::Rts => rejected = true,
                QpState::Sqd => rejected = true,
                QpState::Err => d.state = QpState::Err,
            },
            None => {}
        },
        QpState::Rtr => match i.request {
            Some(req) => match req {
                QpState::Reset => d.state = QpState::Reset,
                QpState::Init => rejected = true,
                // A request for the current state is a no-op, not an error.
                QpState::Rtr => {}
                QpState::Rts => d.state = QpState::Rts,
                QpState::Sqd => rejected = true,
                QpState::Err => d.state = QpState::Err,
            },
            None => {}
        },
        QpState::Rts => match i.request {
            Some(req) => match req {
                QpState::Reset => d.state = QpState::Reset,
                QpState::Init => rejected = true,
                QpState::Rtr => rejected = true,
                // A request for the current state is a no-op, not an error.
                QpState::Rts => {}
                QpState::Sqd => d.state = QpState::Sqd,
                QpState::Err => d.state = QpState::Err,
            },
            None => {}
        },
        QpState::Sqd => match i.request {
            Some(req) => match req {
                QpState::Reset => d.state = QpState::Reset,
                QpState::Init => rejected = true,
                QpState::Rtr => rejected = true,
                QpState::Rts => d.state = QpState::Rts,
                // A request for the current state is a no-op, not an error.
                QpState::Sqd => {}
                QpState::Err => d.state = QpState::Err,
            },
            None => {}
        },
        QpState::Err => match i.request {
            Some(req) => match req {
                QpState::Reset => d.state = QpState::Reset,
                QpState::Init => rejected = true,
                QpState::Rtr => rejected = true,
                QpState::Rts => rejected = true,
                QpState::Sqd => rejected = true,
                // A request for the current state is a no-op, not an error.
                QpState::Err => {}
            },
            None => {}
        },
    }

    // A datapath error is not a request and cannot be refused.
    if i.error {
        d.state = QpState::Err;
        rejected = false;
    }

    o.state = q.state;
    o.can_send = match q.state {
        QpState::Rts => true,
        QpState::Reset => false,
        QpState::Init => false,
        QpState::Rtr => false,
        QpState::Sqd => false,
        QpState::Err => false,
    };
    o.draining = match q.state {
        QpState::Sqd => true,
        QpState::Reset => false,
        QpState::Init => false,
        QpState::Rtr => false,
        QpState::Rts => false,
        QpState::Err => false,
    };
    o.rejected = rejected;

    if cr.reset.any() {
        d.state = QpState::Reset;
        o.state = QpState::Reset;
        o.can_send = false;
        o.draining = false;
        o.rejected = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use rhdl_fpga::doc::render_fsm_diagram_markdown;
    use std::path::PathBuf;

    const ALL: [QpState; 6] = [
        QpState::Reset,
        QpState::Init,
        QpState::Rtr,
        QpState::Rts,
        QpState::Sqd,
        QpState::Err,
    ];

    /// The transition table, as the IBTA defines it, written
    /// independently of the kernel so agreement means something.
    fn legal(from: QpState, to: QpState) -> bool {
        use QpState::*;
        if to == Err {
            return true; // any state may be driven to ERR
        }
        if to == Reset {
            return from != Reset; // teardown from anywhere; RESET->RESET is a no-op
        }
        matches!(
            (from, to),
            (Reset, Init) | (Init, Rtr) | (Rtr, Rts) | (Rts, Sqd) | (Sqd, Rts)
        )
    }

    fn step(from: QpState, request: Option<QpState>, error: bool) -> (Out, D) {
        let cr = ClockReset::dont_care();
        let q = Q { state: from };
        qp_state_machine(cr, In { request, error }, q)
    }

    // ---------------- Tier 1 ----------------

    /// Every (from, to) pair agrees with the independent table: legal
    /// ones take effect, illegal ones are refused and hold.
    #[test]
    fn the_transition_table_matches_the_ibta() {
        for from in ALL {
            for to in ALL {
                let (o, d) = step(from, Some(to), false);
                if from == to {
                    assert_eq!(d.state, from, "{from:?} -> {to:?} should be a no-op");
                    assert!(
                        !o.rejected,
                        "a request for the current state is not an error"
                    );
                } else if legal(from, to) {
                    assert_eq!(
                        d.state, to,
                        "{from:?} -> {to:?} is legal and should take effect"
                    );
                    assert!(!o.rejected, "{from:?} -> {to:?} is legal but was rejected");
                } else {
                    assert_eq!(
                        d.state, from,
                        "{from:?} -> {to:?} is illegal; state must hold"
                    );
                    assert!(
                        o.rejected,
                        "{from:?} -> {to:?} is illegal but was not rejected"
                    );
                }
            }
        }
    }

    /// At least one transition is actually refused, so the test above
    /// is not vacuously passing a table that permits everything.
    #[test]
    fn some_transitions_really_are_illegal() {
        let illegal: Vec<(QpState, QpState)> = ALL
            .iter()
            .flat_map(|f| ALL.iter().map(move |t| (*f, *t)))
            .filter(|(f, t)| f != t && !legal(*f, *t))
            .collect();
        assert!(
            illegal.len() >= 10,
            "only {} illegal pairs; the table is too permissive to be meaningful",
            illegal.len()
        );
        // Spot-check the one that matters most: you cannot jump
        // straight to RTS without the handshake.
        assert!(!legal(QpState::Reset, QpState::Rts));
        let (o, d) = step(QpState::Reset, Some(QpState::Rts), false);
        assert_eq!(d.state, QpState::Reset);
        assert!(o.rejected);
    }

    /// `can_send` is true in RTS and nowhere else. This is the property
    /// the whole widget exists for.
    #[test]
    fn can_send_is_rts_only() {
        for from in ALL {
            let (o, _d) = step(from, None, false);
            assert_eq!(
                o.can_send,
                from == QpState::Rts,
                "{from:?}: can_send should be {}",
                from == QpState::Rts
            );
            assert_eq!(o.draining, from == QpState::Sqd, "{from:?}: draining");
        }
    }

    /// An error forces ERR from any state, overrides a request made on
    /// the same cycle, and is not reported as a rejection.
    #[test]
    fn an_error_overrides_any_request() {
        for from in ALL {
            for req in [None, Some(QpState::Rts), Some(QpState::Reset)] {
                let (o, d) = step(from, req, true);
                assert_eq!(
                    d.state,
                    QpState::Err,
                    "{from:?} with error should go to ERR"
                );
                assert!(!o.rejected, "an error is not a rejected request");
            }
        }
    }

    /// Recovery from ERR is through RESET only.
    #[test]
    fn err_recovers_only_through_reset() {
        for to in ALL {
            let (o, d) = step(QpState::Err, Some(to), false);
            match to {
                QpState::Reset => assert_eq!(d.state, QpState::Reset),
                QpState::Err => assert_eq!(d.state, QpState::Err),
                _ => {
                    assert_eq!(d.state, QpState::Err, "ERR -> {to:?} must be refused");
                    assert!(o.rejected);
                }
            }
        }
    }

    /// No request means no change.
    #[test]
    fn idle_holds_the_state() {
        for from in ALL {
            let (o, d) = step(from, None, false);
            assert_eq!(d.state, from);
            assert!(!o.rejected);
        }
    }

    #[test]
    fn reset_returns_to_the_reset_state() {
        let cr = clock_reset(clock(false), reset(true));
        let q = Q {
            state: QpState::Rts,
        };
        let (o, d) = qp_state_machine(
            cr,
            In {
                request: Some(QpState::Sqd),
                error: true,
            },
            q,
        );
        assert_eq!(d.state, QpState::Reset);
        assert_eq!(o.state, QpState::Reset);
        assert!(!o.can_send);
    }

    // ---------------- Tier 2 ----------------

    /// The bring-up sequence a real connection follows, end to end.
    #[test]
    fn the_bring_up_sequence_reaches_rts() {
        let uut = QpStateMachine::default();
        let stream = bring_up_stimulus()
            .into_iter()
            .with_reset(1)
            .clock_pos_edge(100);
        let states: Vec<QpState> = uut
            .run(stream)
            .synchronous_sample()
            .map(|s| s.output.state)
            .collect();
        // Reset -> Init -> Rtr -> Rts, then drain and come back.
        for want in [
            QpState::Reset,
            QpState::Init,
            QpState::Rtr,
            QpState::Rts,
            QpState::Sqd,
        ] {
            assert!(states.contains(&want), "never reached {want:?}: {states:?}");
        }
        assert!(
            states.iter().any(|s| *s == QpState::Rts),
            "never became ready to send"
        );
    }

    fn bring_up_stimulus() -> Vec<In> {
        let r = |s: Option<QpState>| In {
            request: s,
            error: false,
        };
        vec![
            r(None),
            r(Some(QpState::Init)),
            r(None),
            r(Some(QpState::Rts)), // illegal from INIT: refused
            r(Some(QpState::Rtr)),
            r(Some(QpState::Rts)),
            r(None),
            r(Some(QpState::Sqd)),
            r(None),
            r(Some(QpState::Rts)),
            r(None),
            In {
                request: None,
                error: true,
            },
            r(Some(QpState::Reset)),
            r(None),
        ]
    }

    // ---------------- rule 14: the FSM diagram ----------------

    #[test]
    fn test_fsm_diagram_matches_descriptor() {
        let transitions = rhdl::core::fsm::extract_widget_transitions_strict::<QpStateMachine>()
            .expect("extracting transitions");
        let fresh = render_fsm_diagram_markdown::<QpStateMachine>(&transitions);
        let committed = include_str!("../../doc/qp_state_fsm.md");
        assert_eq!(
            fresh.trim(),
            committed.trim(),
            "doc/qp_state_fsm.md is stale -- re-run `cargo run --example qp_state -p rhdl-roce`"
        );
    }

    /// Every state in the diagram is reachable, and SQE is absent
    /// rather than present-and-unreachable.
    #[test]
    fn every_state_is_reachable_and_there_are_no_orphans() {
        use std::collections::BTreeSet;
        let transitions = rhdl::core::fsm::extract_widget_transitions_strict::<QpStateMachine>()
            .expect("extracting transitions");
        let reached: BTreeSet<usize> = transitions
            .iter()
            .filter(|t| t.source_index != t.target_index)
            .map(|t| t.target_index)
            .collect();
        // RESET (index 0) is the initial state, so it need not be a
        // target -- though teardown makes it one anyway.
        for idx in 1..6 {
            assert!(
                reached.contains(&idx),
                "state index {idx} is unreachable; an orphan state in the diagram is worse \
                 than a missing one"
            );
        }
        assert_eq!(
            ALL.len(),
            6,
            "SQE is deliberately absent; see the module docs"
        );
    }

    // ---------------- Tier 3/4/5 ----------------

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = QpStateMachine::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [4:0] i, output wire [5:0] o);
               wire [8:0] od;
               wire [2:0] d;
               wire [2:0] q;
               assign o = od[5:0];
               top_state c0(.clock_reset(clock_reset), .i(d[2:0]), .o(q[2:0]));
               assign d = od[8:6];
               assign od = kernel_qp_state_machine(clock_reset, i, q);
               function [8:0] kernel_qp_state_machine(input reg [1:0] arg_0, input reg [4:0] arg_1, input reg [2:0] arg_2);
                     reg [2:0] r0;
                     // d
                     reg [2:0] r1;
                     reg [3:0] r2;
                     reg [4:0] r3;
                     reg [0:0] r4;
                     reg [2:0] r5;
                     // d
                     reg [2:0] r6;
                     // d
                     reg [2:0] r7;
                     // d
                     reg [2:0] r8;
                     // rejected
                     reg [0:0] r9;
                     // d
                     reg [2:0] r10;
                     // rejected
                     reg [0:0] r11;
                     reg [3:0] r12;
                     reg [0:0] r13;
                     reg [2:0] r14;
                     // d
                     reg [2:0] r15;
                     // d
                     reg [2:0] r16;
                     // d
                     reg [2:0] r17;
                     // d
                     reg [2:0] r18;
                     // rejected
                     reg [0:0] r19;
                     // d
                     reg [2:0] r20;
                     // rejected
                     reg [0:0] r21;
                     reg [3:0] r22;
                     reg [0:0] r23;
                     reg [2:0] r24;
                     // d
                     reg [2:0] r25;
                     // d
                     reg [2:0] r26;
                     // d
                     reg [2:0] r27;
                     // d
                     reg [2:0] r28;
                     // rejected
                     reg [0:0] r29;
                     // d
                     reg [2:0] r30;
                     // rejected
                     reg [0:0] r31;
                     reg [3:0] r32;
                     reg [0:0] r33;
                     reg [2:0] r34;
                     // d
                     reg [2:0] r35;
                     // d
                     reg [2:0] r36;
                     // d
                     reg [2:0] r37;
                     // d
                     reg [2:0] r38;
                     // rejected
                     reg [0:0] r39;
                     // d
                     reg [2:0] r40;
                     // rejected
                     reg [0:0] r41;
                     reg [3:0] r42;
                     reg [0:0] r43;
                     reg [2:0] r44;
                     // d
                     reg [2:0] r45;
                     // d
                     reg [2:0] r46;
                     // d
                     reg [2:0] r47;
                     // d
                     reg [2:0] r48;
                     // rejected
                     reg [0:0] r49;
                     // d
                     reg [2:0] r50;
                     // rejected
                     reg [0:0] r51;
                     reg [3:0] r52;
                     reg [0:0] r53;
                     reg [2:0] r54;
                     // d
                     reg [2:0] r55;
                     // d
                     reg [2:0] r56;
                     // rejected
                     reg [0:0] r57;
                     // d
                     reg [2:0] r58;
                     // rejected
                     reg [0:0] r59;
                     // d
                     reg [2:0] r60;
                     // rejected
                     reg [0:0] r61;
                     reg [0:0] r62;
                     // d
                     reg [2:0] r63;
                     // d
                     reg [2:0] r64;
                     // rejected
                     reg [0:0] r65;
                     // o
                     reg [5:0] r66;
                     reg [0:0] r67;
                     // o
                     reg [5:0] r68;
                     reg [0:0] r69;
                     // o
                     reg [5:0] r70;
                     // o
                     reg [5:0] r71;
                     reg [0:0] r72;
                     reg [1:0] r73;
                     reg [0:0] r74;
                     // d
                     reg [2:0] r75;
                     // o
                     reg [5:0] r76;
                     // o
                     reg [5:0] r77;
                     // o
                     reg [5:0] r78;
                     // o
                     reg [5:0] r79;
                     // d
                     reg [2:0] r80;
                     // o
                     reg [5:0] r81;
                     reg [8:0] r82;
                     localparam l0 = 3'bXXX;
                     localparam l1 = 3'b001;
                     localparam l2 = 3'b101;
                     localparam l3 = 3'b000;
                     localparam l4 = 3'b001;
                     localparam l5 = 3'b010;
                     localparam l6 = 3'b011;
                     localparam l7 = 3'b100;
                     localparam l8 = 3'b101;
                     localparam l9 = 1'b0;
                     localparam l10 = 1'b1;
                     localparam l11 = 1'b1;
                     localparam l12 = 1'b1;
                     localparam l13 = 1'b1;
                     localparam l14 = 1'b0;
                     localparam l15 = 3'b000;
                     localparam l16 = 3'b010;
                     localparam l17 = 3'b101;
                     localparam l18 = 3'b000;
                     localparam l19 = 3'b001;
                     localparam l20 = 3'b010;
                     localparam l21 = 3'b011;
                     localparam l22 = 3'b100;
                     localparam l23 = 3'b101;
                     localparam l24 = 1'b1;
                     localparam l25 = 1'b1;
                     localparam l26 = 1'b1;
                     localparam l27 = 1'b0;
                     localparam l28 = 3'b000;
                     localparam l29 = 3'b011;
                     localparam l30 = 3'b101;
                     localparam l31 = 3'b000;
                     localparam l32 = 3'b001;
                     localparam l33 = 3'b010;
                     localparam l34 = 3'b011;
                     localparam l35 = 3'b100;
                     localparam l36 = 3'b101;
                     localparam l37 = 1'b1;
                     localparam l38 = 1'b1;
                     localparam l39 = 1'b1;
                     localparam l40 = 1'b0;
                     localparam l41 = 3'b000;
                     localparam l42 = 3'b100;
                     localparam l43 = 3'b101;
                     localparam l44 = 3'b000;
                     localparam l45 = 3'b001;
                     localparam l46 = 3'b010;
                     localparam l47 = 3'b011;
                     localparam l48 = 3'b100;
                     localparam l49 = 3'b101;
                     localparam l50 = 1'b1;
                     localparam l51 = 1'b1;
                     localparam l52 = 1'b1;
                     localparam l53 = 1'b0;
                     localparam l54 = 3'b000;
                     localparam l55 = 3'b011;
                     localparam l56 = 3'b101;
                     localparam l57 = 3'b000;
                     localparam l58 = 3'b001;
                     localparam l59 = 3'b010;
                     localparam l60 = 3'b011;
                     localparam l61 = 3'b100;
                     localparam l62 = 3'b101;
                     localparam l63 = 1'b1;
                     localparam l64 = 1'b1;
                     localparam l65 = 1'b1;
                     localparam l66 = 1'b0;
                     localparam l67 = 3'b000;
                     localparam l68 = 3'b000;
                     localparam l69 = 3'b001;
                     localparam l70 = 3'b010;
                     localparam l71 = 3'b011;
                     localparam l72 = 3'b100;
                     localparam l73 = 3'b101;
                     localparam l74 = 1'b1;
                     localparam l75 = 1'b1;
                     localparam l76 = 1'b1;
                     localparam l77 = 1'b1;
                     localparam l78 = 1'b1;
                     localparam l79 = 1'b0;
                     localparam l80 = 3'b000;
                     localparam l81 = 3'b001;
                     localparam l82 = 3'b010;
                     localparam l83 = 3'b011;
                     localparam l84 = 3'b100;
                     localparam l85 = 3'b101;
                     localparam l86 = 3'b101;
                     localparam l87 = 1'b0;
                     localparam l88 = 6'bXXXXXX;
                     localparam l89 = 3'b011;
                     localparam l90 = 1'b1;
                     localparam l91 = 3'b000;
                     localparam l92 = 1'b0;
                     localparam l93 = 3'b001;
                     localparam l94 = 1'b0;
                     localparam l95 = 3'b010;
                     localparam l96 = 1'b0;
                     localparam l97 = 3'b100;
                     localparam l98 = 1'b0;
                     localparam l99 = 3'b101;
                     localparam l100 = 1'b0;
                     localparam l101 = 3'b100;
                     localparam l102 = 1'b1;
                     localparam l103 = 3'b000;
                     localparam l104 = 1'b0;
                     localparam l105 = 3'b001;
                     localparam l106 = 1'b0;
                     localparam l107 = 3'b010;
                     localparam l108 = 1'b0;
                     localparam l109 = 3'b011;
                     localparam l110 = 1'b0;
                     localparam l111 = 3'b101;
                     localparam l112 = 1'b0;
                     localparam l113 = 3'b000;
                     localparam l114 = 3'b000;
                     localparam l115 = 1'b0;
                     localparam l116 = 1'b0;
                     localparam l117 = 1'b0;
                     begin
                        r73 = arg_0;
                        r3 = arg_1;
                        r0 = arg_2;
                        r1 = l0;
                        r1[2:0] = r0;
                        r2 = r3[3:0];
                        r4 = r2[3:3];
                        r5 = r2[2:0];
                        r6 = r1;
                        r6[2:0] = l1;
                        r7 = r1;
                        r7[2:0] = l2;
                        case (r5)
                           3'b000 : r8 = r1;
                           3'b001 : r8 = r6;
                           3'b010 : r8 = r1;
                           3'b011 : r8 = r1;
                           3'b100 : r8 = r1;
                           3'b101 : r8 = r7;
                        endcase
                        case (r5)
                           3'b000 : r9 = l9;
                           3'b001 : r9 = l9;
                           3'b010 : r9 = l10;
                           3'b011 : r9 = l11;
                           3'b100 : r9 = l12;
                           3'b101 : r9 = l9;
                        endcase
                        case (r4)
                           1'b1 : r10 = r8;
                           1'b0 : r10 = r1;
                        endcase
                        case (r4)
                           1'b1 : r11 = r9;
                           1'b0 : r11 = l9;
                        endcase
                        r12 = r3[3:0];
                        r13 = r12[3:3];
                        r14 = r12[2:0];
                        r15 = r1;
                        r15[2:0] = l15;
                        r16 = r1;
                        r16[2:0] = l16;
                        r17 = r1;
                        r17[2:0] = l17;
                        case (r14)
                           3'b000 : r18 = r15;
                           3'b001 : r18 = r1;
                           3'b010 : r18 = r16;
                           3'b011 : r18 = r1;
                           3'b100 : r18 = r1;
                           3'b101 : r18 = r17;
                        endcase
                        case (r14)
                           3'b000 : r19 = l9;
                           3'b001 : r19 = l9;
                           3'b010 : r19 = l9;
                           3'b011 : r19 = l24;
                           3'b100 : r19 = l25;
                           3'b101 : r19 = l9;
                        endcase
                        case (r13)
                           1'b1 : r20 = r18;
                           1'b0 : r20 = r1;
                        endcase
                        case (r13)
                           1'b1 : r21 = r19;
                           1'b0 : r21 = l9;
                        endcase
                        r22 = r3[3:0];
                        r23 = r22[3:3];
                        r24 = r22[2:0];
                        r25 = r1;
                        r25[2:0] = l28;
                        r26 = r1;
                        r26[2:0] = l29;
                        r27 = r1;
                        r27[2:0] = l30;
                        case (r24)
                           3'b000 : r28 = r25;
                           3'b001 : r28 = r1;
                           3'b010 : r28 = r1;
                           3'b011 : r28 = r26;
                           3'b100 : r28 = r1;
                           3'b101 : r28 = r27;
                        endcase
                        case (r24)
                           3'b000 : r29 = l9;
                           3'b001 : r29 = l37;
                           3'b010 : r29 = l9;
                           3'b011 : r29 = l9;
                           3'b100 : r29 = l38;
                           3'b101 : r29 = l9;
                        endcase
                        case (r23)
                           1'b1 : r30 = r28;
                           1'b0 : r30 = r1;
                        endcase
                        case (r23)
                           1'b1 : r31 = r29;
                           1'b0 : r31 = l9;
                        endcase
                        r32 = r3[3:0];
                        r33 = r32[3:3];
                        r34 = r32[2:0];
                        r35 = r1;
                        r35[2:0] = l41;
                        r36 = r1;
                        r36[2:0] = l42;
                        r37 = r1;
                        r37[2:0] = l43;
                        case (r34)
                           3'b000 : r38 = r35;
                           3'b001 : r38 = r1;
                           3'b010 : r38 = r1;
                           3'b011 : r38 = r1;
                           3'b100 : r38 = r36;
                           3'b101 : r38 = r37;
                        endcase
                        case (r34)
                           3'b000 : r39 = l9;
                           3'b001 : r39 = l50;
                           3'b010 : r39 = l51;
                           3'b011 : r39 = l9;
                           3'b100 : r39 = l9;
                           3'b101 : r39 = l9;
                        endcase
                        case (r33)
                           1'b1 : r40 = r38;
                           1'b0 : r40 = r1;
                        endcase
                        case (r33)
                           1'b1 : r41 = r39;
                           1'b0 : r41 = l9;
                        endcase
                        r42 = r3[3:0];
                        r43 = r42[3:3];
                        r44 = r42[2:0];
                        r45 = r1;
                        r45[2:0] = l54;
                        r46 = r1;
                        r46[2:0] = l55;
                        r47 = r1;
                        r47[2:0] = l56;
                        case (r44)
                           3'b000 : r48 = r45;
                           3'b001 : r48 = r1;
                           3'b010 : r48 = r1;
                           3'b011 : r48 = r46;
                           3'b100 : r48 = r1;
                           3'b101 : r48 = r47;
                        endcase
                        case (r44)
                           3'b000 : r49 = l9;
                           3'b001 : r49 = l63;
                           3'b010 : r49 = l64;
                           3'b011 : r49 = l9;
                           3'b100 : r49 = l9;
                           3'b101 : r49 = l9;
                        endcase
                        case (r43)
                           1'b1 : r50 = r48;
                           1'b0 : r50 = r1;
                        endcase
                        case (r43)
                           1'b1 : r51 = r49;
                           1'b0 : r51 = l9;
                        endcase
                        r52 = r3[3:0];
                        r53 = r52[3:3];
                        r54 = r52[2:0];
                        r55 = r1;
                        r55[2:0] = l67;
                        case (r54)
                           3'b000 : r56 = r55;
                           3'b001 : r56 = r1;
                           3'b010 : r56 = r1;
                           3'b011 : r56 = r1;
                           3'b100 : r56 = r1;
                           3'b101 : r56 = r1;
                        endcase
                        case (r54)
                           3'b000 : r57 = l9;
                           3'b001 : r57 = l74;
                           3'b010 : r57 = l75;
                           3'b011 : r57 = l76;
                           3'b100 : r57 = l77;
                           3'b101 : r57 = l9;
                        endcase
                        case (r53)
                           1'b1 : r58 = r56;
                           1'b0 : r58 = r1;
                        endcase
                        case (r53)
                           1'b1 : r59 = r57;
                           1'b0 : r59 = l9;
                        endcase
                        case (r0)
                           3'b000 : r60 = r10;
                           3'b001 : r60 = r20;
                           3'b010 : r60 = r30;
                           3'b011 : r60 = r40;
                           3'b100 : r60 = r50;
                           3'b101 : r60 = r58;
                        endcase
                        case (r0)
                           3'b000 : r61 = r11;
                           3'b001 : r61 = r21;
                           3'b010 : r61 = r31;
                           3'b011 : r61 = r41;
                           3'b100 : r61 = r51;
                           3'b101 : r61 = r59;
                        endcase
                        r62 = r3[4:4];
                        r63 = r60;
                        r63[2:0] = l86;
                        r64 = r62 ? r63 : r60;
                        r65 = r62 ? l87 : r61;
                        r66 = l88;
                        r66[2:0] = r0;
                        case (r0)
                           3'b011 : r67 = l90;
                           3'b000 : r67 = l92;
                           3'b001 : r67 = l94;
                           3'b010 : r67 = l96;
                           3'b100 : r67 = l98;
                           3'b101 : r67 = l100;
                        endcase
                        r68 = r66;
                        r68[3:3] = r67;
                        case (r0)
                           3'b100 : r69 = l102;
                           3'b000 : r69 = l104;
                           3'b001 : r69 = l106;
                           3'b010 : r69 = l108;
                           3'b011 : r69 = l110;
                           3'b101 : r69 = l112;
                        endcase
                        r70 = r68;
                        r70[4:4] = r69;
                        r71 = r70;
                        r71[5:5] = r65;
                        r72 = r73[1:1];
                        r74 = |r72;
                        r75 = r64;
                        r75[2:0] = l113;
                        r76 = r71;
                        r76[2:0] = l114;
                        r77 = r76;
                        r77[3:3] = l115;
                        r78 = r77;
                        r78[4:4] = l116;
                        r79 = r78;
                        r79[5:5] = l117;
                        r80 = r74 ? r75 : r64;
                        r81 = r74 ? r79 : r71;
                        r82 = {r80, r81};
                        kernel_qp_state_machine = r82;
                     end
               endfunction
            endmodule
            module top_state(input wire [1:0] clock_reset, input wire [2:0] i, output reg [2:0] o);
               wire  clock;
               wire  reset;
               assign clock = clock_reset[0];
               assign reset = clock_reset[1];
               initial begin
                  o = 3'b000;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     o <= 3'b000;
                  end else begin
                     o <= i;
                  end
               end
            endmodule
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    #[test]
    fn test_qp_state_hdl_works() -> miette::Result<()> {
        let uut = QpStateMachine::default();
        let stream = bring_up_stimulus()
            .into_iter()
            .with_reset(1)
            .clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_qp_state_trace() -> miette::Result<()> {
        let uut = QpStateMachine::default();
        let stream = bring_up_stimulus()
            .into_iter()
            .with_reset(1)
            .clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("qp_state");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["9a1aae1d24f74fd443d62e5e962df5b0fca913ba67504a834f8a764d4ec8d559"];
        let digest = vcd.dump_to_file(root.join("qp_state.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
