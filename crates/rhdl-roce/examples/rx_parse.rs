// A RoCEv2 RDMA WRITE frame, parsed back into its headers.
//
// The frame is built here with the same pure functions the transmitter
// uses -- `bth_from_plan` and `header_byte` -- so no hex is typed by
// hand and the input is exactly what this crate emits. 70 header bytes
// (a WRITE_ONLY carries a RETH), 16 payload bytes, 4 trailer bytes.
//
// What to look for:
//
//   - `phase` walks Idle -> Header -> Payload -> Trailer -> Idle with
//     no length signal anywhere. Everything it needs comes out of the
//     IPv4 total-length field at offset 17. `busy` follows one cycle
//     behind `start_of_frame`, because both report the *registered*
//     phase: during the cycle byte 0 arrives the parser is still Idle.
//   - `headers` is `None` for the first 70 cycles and then holds the
//     recovered BTH and RETH for the rest of the frame. The cycle it
//     appears is the cycle the first payload byte appears -- which is
//     the point: the RETH address is known before the first byte it
//     addresses.
//   - `payload` carries exactly the 16 payload bytes and neither the
//     headers before them nor the trailer after.
//   - `done` is one cycle, at the last byte, and `verdict` is `Accept`.
//
// The four trailer bytes here are a recognisable placeholder rather
// than a real ICRC, because this widget deliberately does not check
// it: it reports the received value on `icrc` and comparison is a
// separate step. `tests/` closes that loop with real checksums.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::{render_fsm_diagram_markdown, render_svg_markdown};
use rhdl_roce::frame::{FrameConfig, bth_from_plan, header_byte, header_bytes};
use rhdl_roce::headers::{Reth, Transport, WriteOpcode};
use rhdl_roce::qp::context::{Pmtu, QpContext};
use rhdl_roce::qp::segmenter::PacketPlan;
use rhdl_roce::rx::parse::{In, RxParser};

fn main() -> Result<(), RHDLError> {
    let doc = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("doc");
    std::fs::create_dir_all(&doc).expect("creating doc/");

    // --- the state diagram, derived from the kernel's own RHIF.
    let transitions = rhdl::core::fsm::extract_widget_transitions_strict::<RxParser>()
        .expect("extracting the FSM transitions");
    std::fs::write(
        doc.join("rx_parse_fsm.md"),
        render_fsm_diagram_markdown::<RxParser>(&transitions),
    )
    .expect("writing doc/rx_parse_fsm.md");

    // --- a frame to parse, built the same way one is sent.
    let cfg = FrameConfig {
        dst_mac: bits(0x000a_3501_0203),
        src_mac: bits(0xec0d_9aaa_bbcc),
        src_ip: bits(0xC0A8_010A),
        dst_ip: bits(0xC0A8_0132),
        src_port: bits(0x1234),
        dst_port: bits(4791),
        ip_id: bits(0),
        ttl: bits(64),
        dscp_ecn: bits(0),
    };
    let ctx = QpContext {
        dqpn: bits(0x000111),
        transport: Transport::Uc,
        pmtu: Pmtu::B1024,
        rkey: bits(0x0000_A5A5),
        pkey: bits(0xFFFF),
    };
    let plan = PacketPlan {
        opcode: WriteOpcode::Only,
        psn: bits(1),
        payload_bytes: bits(16),
        padcount: bits(0),
        carries_reth: true,
        reth: Reth {
            va: bits(0x0000_7F00_1234_0000),
            rkey: bits(0x0000_A5A5),
            dma_len: bits(16),
        },
    };
    let bth = bth_from_plan(ctx, plan, false);

    let header_len = header_bytes(plan).raw() as usize;
    let mut frame: Vec<u8> = (0..header_len)
        .map(|k| header_byte(cfg, bth, plan, bits(k as u128)).raw() as u8)
        .collect();
    frame.extend((0..16u8).map(|k| 0xA0 | k));
    frame.extend([0xDE, 0xAD, 0xBE, 0xEF]);

    // --- drive it in, one byte per cycle.
    let idle = In {
        data: None,
        start_of_frame: false,
        end_of_frame: false,
    };
    let last = frame.len() - 1;
    let mut stream = vec![idle];
    for (k, b) in frame.iter().enumerate() {
        stream.push(In {
            data: Some(bits(*b as u128)),
            start_of_frame: k == 0,
            end_of_frame: k == last,
        });
    }
    stream.push(idle);
    stream.push(idle);

    let uut = RxParser::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(input).collect::<SvgFile>();
    let markdown = render_svg_markdown(
        vcd,
        SvgOptions::default().with_io_filter().with_label_width(26),
    )
    .expect("rendering the waveform");
    std::fs::write(doc.join("rx_parse.md"), markdown).expect("writing doc/rx_parse.md");
    Ok(())
}
