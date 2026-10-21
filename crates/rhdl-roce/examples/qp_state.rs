// A queue pair being brought up, drained, failed and torn down.
//
// What to look for:
//
//   - The bring-up sequence is RESET -> INIT -> RTR -> RTS. Each step
//     is a separate request from the control plane; the hardware will
//     not skip one.
//   - The fourth request asks for RTS while still in INIT. It is
//     *refused*: `rejected` pulses, and `state` does not move. An
//     illegal request is a control-plane bug, and conflating it with a
//     wire error -- by entering ERR -- would make the two
//     indistinguishable when they want different responses.
//   - `can_send` is asserted only in RTS. That is the whole point of
//     the widget: before it existed, the transmitter would build frames
//     for a connection the peer had never heard of.
//   - RTS -> SQD -> RTS is the drain cycle, used to quiesce a
//     connection without tearing it down. `draining` marks it.
//   - Then `error` is asserted. It forces ERR from any state and
//     overrides any request on the same cycle, because a wire error is
//     not something the control plane gets to disagree with. Recovery
//     is through RESET and nowhere else.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::{render_fsm_diagram_markdown, render_svg_markdown};
use rhdl_roce::qp::state::{In, QpState, QpStateMachine};

fn main() -> Result<(), RHDLError> {
    let doc = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("doc");
    std::fs::create_dir_all(&doc).expect("creating doc/");

    // The diagram, derived from the kernel's own RHIF.
    let transitions = rhdl::core::fsm::extract_widget_transitions_strict::<QpStateMachine>()
        .expect("extracting the FSM transitions");
    std::fs::write(
        doc.join("qp_state_fsm.md"),
        render_fsm_diagram_markdown::<QpStateMachine>(&transitions),
    )
    .expect("writing doc/qp_state_fsm.md");

    let r = |s: Option<QpState>| In {
        request: s,
        error: false,
    };
    let stream = vec![
        r(None),
        r(Some(QpState::Init)),
        r(None),
        r(Some(QpState::Rts)), // illegal from INIT -- refused
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
    ];

    let uut = QpStateMachine::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(input).collect::<SvgFile>();
    let markdown = render_svg_markdown(
        vcd,
        SvgOptions::default().with_io_filter().with_label_width(22),
    )
    .expect("rendering the waveform");
    std::fs::write(doc.join("qp_state.md"), markdown).expect("writing doc/qp_state.md");
    Ok(())
}
