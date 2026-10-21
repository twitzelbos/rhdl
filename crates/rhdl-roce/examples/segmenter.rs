// One 600-byte RDMA WRITE message, segmented at a 256-byte path MTU.
//
// 600 bytes does not divide by 256, so this is the general case: a
// FIRST, one MIDDLE, and a ragged LAST. The three packets correspond to
// vectors `uc_write_first_256`, `uc_write_middle_256` and
// `uc_write_last_88` in `tests/vectors/write_vectors.txt`, so the
// opcodes and sequence numbers in this trace are cross-checked against
// Scapy-generated packets.
//
// What to look for:
//
//   - `start` is asserted for one cycle. Everything after that is the
//     widget working through the message on its own.
//   - `plan` carries the opcode, and it walks First -> Middle -> Last.
//     Only the first packet has `carries_reth` set: the responder
//     tracks the running address for the rest.
//   - `psn` increments by one per packet, from the 0x10 seed.
//   - The RETH's `dma_len` is 600 on the FIRST packet -- the *whole*
//     message, not that packet's 256 bytes. This is the field most
//     easily got wrong.
//   - `ready` is held low on one cycle in the middle. The plan does not
//     advance and is not dropped: the same packet is still presented on
//     the next cycle. That is the backpressure contract.
//   - `busy` falls after the LAST packet is accepted.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::render_svg_markdown;
use rhdl_roce::headers::Transport;
use rhdl_roce::qp::context::{MessageRequest, Pmtu, QpContext};
use rhdl_roce::qp::segmenter::{In, WriteSegmenter};

fn main() -> Result<(), RHDLError> {
    let idle = In {
        start: false,
        msg: MessageRequest {
            va: bits(0x0000_7F00_1234_0000),
            length: bits(600),
        },
        ctx: QpContext {
            dqpn: bits(0x000111),
            transport: Transport::Uc,
            pmtu: Pmtu::B256,
            rkey: bits(0x0000_A5A5),
            pkey: bits(0xFFFF),
        },
        psn_start: bits(0x10),
        ready: true,
    };

    let mut stream = vec![In {
        start: true,
        ..idle
    }];
    for n in 0..8 {
        // One not-ready cycle, to show the plan being held.
        stream.push(In {
            ready: n != 2,
            ..idle
        });
    }

    let uut = WriteSegmenter::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(input).collect::<SvgFile>();

    let markdown = render_svg_markdown(
        vcd,
        SvgOptions::default().with_io_filter().with_label_width(24),
    )
    .expect("rendering the waveform");
    let doc = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("doc");
    std::fs::create_dir_all(&doc).expect("creating doc/");
    std::fs::write(doc.join("segmenter.md"), markdown).expect("writing doc/segmenter.md");
    Ok(())
}
