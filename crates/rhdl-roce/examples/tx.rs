// A complete RoCEv2 RDMA WRITE frame, emitted byte by byte.
//
// One UC RDMA_WRITE_ONLY carrying a 16-byte payload to queue pair
// 0x000111 at PSN 1 -- the same shape as vector `uc_write_only_16` in
// `tests/vectors/write_vectors.txt`. 70 header bytes (the RETH is
// present, because ONLY carries one), 16 payload bytes, 4 ICRC bytes:
// 90 cycles of `data`, one byte per cycle.
//
// What to look for:
//
//   - `start` is asserted for one cycle. Everything after is the widget
//     working through the message on its own: the segmenter decides the
//     packet, the frame generator produces header bytes, the ICRC
//     engine folds each byte as it goes out.
//   - `sof` marks byte 0 and `eof` marks the last ICRC byte. Those are
//     the framing markers a MAC or a DDR-ring adapter needs; there is no
//     separate length signal.
//   - `payload_advance` is asserted for exactly the 16 payload cycles,
//     which is how the caller's memory source is stepped.
//   - The four bytes after the payload are the ICRC, and they need no
//     stall: the engine was fed each covered byte as it was emitted, so
//     the checksum is already settled by the time the phase changes.
//   - `ready` goes low for one cycle mid-payload. `data` holds the same
//     byte and nothing advances -- not the index, not the ICRC. That is
//     the backpressure contract.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::{render_fsm_diagram_markdown, render_svg_markdown};
use rhdl_roce::frame::FrameConfig;
use rhdl_roce::headers::Transport;
use rhdl_roce::qp::context::{MessageRequest, Pmtu, QpContext};
use rhdl_roce::tx::{In, RoceWriteTx};

fn doc_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("doc")
}

fn main() -> Result<(), RHDLError> {
    let doc = doc_dir();
    std::fs::create_dir_all(&doc).expect("creating doc/");

    // --- the state diagram, derived from the kernel's own RHIF.
    //
    // `rhdl_fpga::doc::write_fsm_diagram` would be the one-liner, but it
    // resolves its output path against rhdl-fpga's manifest directory,
    // so a sibling crate has to render and write it itself.
    let transitions = rhdl::core::fsm::extract_widget_transitions_strict::<RoceWriteTx>()
        .expect("extracting the FSM transitions");
    std::fs::write(
        doc.join("tx_fsm.md"),
        render_fsm_diagram_markdown::<RoceWriteTx>(&transitions),
    )
    .expect("writing doc/tx_fsm.md");

    // --- and the waveform.
    let idle = In {
        start: false,
        msg: MessageRequest {
            va: bits(0x0000_7F00_1234_0000),
            length: bits(16),
        },
        ctx: QpContext {
            dqpn: bits(0x000111),
            transport: Transport::Uc,
            pmtu: Pmtu::B1024,
            rkey: bits(0x0000_A5A5),
            pkey: bits(0xFFFF),
        },
        cfg: FrameConfig {
            dst_mac: bits(0xec0d_9aaa_bbcc),
            src_mac: bits(0x000a_3501_0203),
            src_ip: bits(0xC0A8_0132),
            dst_ip: bits(0xC0A8_010A),
            src_port: bits(0x1234),
            dst_port: bits(4791),
            ip_id: bits(0),
            ttl: bits(64),
            dscp_ecn: bits(0),
        },
        psn_start: bits(1),
        ackreq: false,
        payload_byte: bits(0),
        ready: true,
    };

    let mut stream = vec![In {
        start: true,
        ..idle
    }];
    let mut payload_pos = 0u128;
    for n in 0..100 {
        // One not-ready cycle, to show the hold.
        let ready = n != 80;
        stream.push(In {
            payload_byte: bits(payload_pos & 0xFF),
            ready,
            ..idle
        });
        // The real caller advances on `payload_advance`; this open-loop
        // approximation is enough for a trace and keeps the example
        // short. `tests` drives it closed-loop.
        if ready && (72..88).contains(&n) {
            payload_pos += 1;
        }
    }

    let uut = RoceWriteTx::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(input).collect::<SvgFile>();
    let markdown = render_svg_markdown(
        vcd,
        SvgOptions::default().with_io_filter().with_label_width(26),
    )
    .expect("rendering the waveform");
    std::fs::write(doc.join("tx.md"), markdown).expect("writing doc/tx.md");
    Ok(())
}
