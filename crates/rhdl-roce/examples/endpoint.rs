// A queue pair brought up, two messages queued, both sent.
//
// What to look for:
//
//   - The first cycles configure the table (`table_write`) and walk the
//     state machine RESET -> INIT -> RTR -> RTS. Nothing is transmitted
//     during that, even though work has already been posted: posting is
//     allowed in any state, sending is not.
//   - `can_send` rises on reaching RTS, and only then does `data` start
//     flowing. That gate is the queue-pair state machine doing its job;
//     before Phase 3 the transmitter would have framed for a connection
//     the peer had never heard of.
//   - Two `sof`/`eof` pairs: two complete frames, back to back, from one
//     posting burst. Before the work queue existed the second `start`
//     would have abandoned the first message.
//   - `busy` falls only after the second frame's last byte.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::render_svg_markdown;
use rhdl_roce::endpoint::{In, RoceWriteEndpoint};
use rhdl_roce::frame::{FrameConfig, ROCE_UDP_PORT};
use rhdl_roce::headers::Transport;
use rhdl_roce::qp::context::{MessageRequest, Pmtu, QpContext, WorkRequest};
use rhdl_roce::qp::state::QpState;
use rhdl_roce::qp::table::QpWrite;

const QPN: u128 = 0x000111;

fn cfg() -> FrameConfig {
    FrameConfig {
        dst_mac: bits(0xec0d_9aaa_bbcc),
        src_mac: bits(0x000a_3501_0203),
        src_ip: bits(0xC0A8_0132),
        dst_ip: bits(0xC0A8_010A),
        src_port: bits(0x1234),
        dst_port: bits(ROCE_UDP_PORT),
        ip_id: bits(0),
        ttl: bits(64),
        dscp_ecn: bits(0),
    }
}

fn ctx() -> QpContext {
    QpContext {
        dqpn: bits(QPN),
        transport: Transport::Uc,
        pmtu: Pmtu::B1024,
        rkey: bits(0x0000_A5A5),
        pkey: bits(0xFFFF),
    }
}

fn main() -> Result<(), RHDLError> {
    let idle = In {
        qp_request: None,
        qp_error: false,
        table_write: None,
        post: None,
        psn_start: bits(1),
        ackreq: false,
        cfg: cfg(),
        payload_byte: bits(0),
        ready: true,
    };
    let msg = WorkRequest {
        qpn: bits(QPN),
        msg: MessageRequest {
            va: bits(0x0000_7F00_1234_0000),
            length: bits(16),
        },
    };

    let mut stream = vec![
        // Configure the context, and post work before we are ready.
        In {
            table_write: Some(QpWrite {
                qpn: bits(QPN),
                ctx: Some(ctx()),
            }),
            post: Some(msg),
            ..idle
        },
        In {
            post: Some(msg),
            qp_request: Some(QpState::Init),
            ..idle
        },
        In {
            qp_request: Some(QpState::Rtr),
            ..idle
        },
        In {
            qp_request: Some(QpState::Rts),
            ..idle
        },
    ];
    for _ in 0..210 {
        stream.push(idle);
    }

    let uut = RoceWriteEndpoint::<4, 3>::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(input).collect::<SvgFile>();
    let markdown = render_svg_markdown(
        vcd,
        SvgOptions::default().with_io_filter().with_label_width(26),
    )
    .expect("rendering the waveform");
    let doc = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("doc");
    std::fs::create_dir_all(&doc).expect("creating doc/");
    std::fs::write(doc.join("endpoint.md"), markdown).expect("writing doc/endpoint.md");
    Ok(())
}
