// Three messages posted back to back, drained one at a time, with a
// post refused mid-drain.
//
// What to look for:
//
//   - `post` offers a message and `accepted` reports whether it was
//     taken. A requester that ignores `accepted` loses work; one that
//     honours it cannot.
//   - `current` holds the head *stable* across many idle cycles. It
//     advances only when `complete` strobes -- which the send path
//     asserts after the last packet of a message, not after the first.
//     A queue that advanced on a ready signal would discard the rest of
//     a multi-packet message.
//   - Partway through, `flush` is asserted with a post offered.
//     `accepted` stays low and the head keeps draining: that is how
//     SQD ("finish what is outstanding, accept nothing new") is
//     implemented, and it lives here rather than in the state machine.
//   - `empty` is derived from the head being `None` rather than being a
//     separate flag, so the two cannot disagree.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::render_svg_markdown;
use rhdl_roce::qp::context::MessageRequest;
use rhdl_roce::qp::work_queue::{In, WorkQueue};

fn msg(n: u128) -> MessageRequest {
    MessageRequest {
        va: bits(0x1000 + n * 0x100),
        length: bits(n * 16),
    }
}

fn main() -> Result<(), RHDLError> {
    let idle = In {
        post: None,
        complete: false,
        flush: false,
    };
    let mut stream = Vec::new();
    for n in 1..=3u128 {
        stream.push(In {
            post: Some(msg(n)),
            ..idle
        });
    }
    stream.push(idle);
    stream.push(In {
        complete: true,
        ..idle
    });
    stream.push(idle);
    // A post offered while flushing: refused.
    stream.push(In {
        post: Some(msg(9)),
        complete: false,
        flush: true,
    });
    stream.push(In {
        complete: true,
        ..idle
    });
    stream.push(idle);
    stream.push(In {
        complete: true,
        ..idle
    });
    stream.push(idle);

    let uut = WorkQueue::<MessageRequest, 3>::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(input).collect::<SvgFile>();
    let markdown = render_svg_markdown(
        vcd,
        SvgOptions::default().with_io_filter().with_label_width(22),
    )
    .expect("rendering the waveform");
    let doc = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("doc");
    std::fs::create_dir_all(&doc).expect("creating doc/");
    std::fs::write(doc.join("work_queue.md"), markdown).expect("writing doc/work_queue.md");
    Ok(())
}
