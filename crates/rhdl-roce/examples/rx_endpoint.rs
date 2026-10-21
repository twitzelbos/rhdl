// An RDMA WRITE received and placed in local memory.
//
// A memory region is registered, a queue pair configured, and then one
// WRITE_ONLY frame arrives carrying 16 bytes.  The frame is the
// committed Scapy vector, so its ICRC is a real one.
//
// What to look for:
//
//   - the two configuration writes go in first, and nothing happens for
//     a couple of cycles while the RAMs settle.  A read issued to a
//     synchronous RAM answers on the next cycle, which is the whole
//     reason the placement path runs one cycle behind the wire.
//   - `write` carries one byte and one address per cycle, starting one
//     cycle after the first payload byte reached the parser.  The
//     addresses are `local_base + offset`, not the virtual addresses the
//     sender used -- the memory region translated them.
//   - `outcome` is `Placed` at `done`, and `placed` counts 16.
//   - the write stream stops before the four ICRC bytes, because those
//     are not payload.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::render_svg_markdown;
use rhdl_roce::headers::Transport;
use rhdl_roce::qp::context::{Pmtu, QpContext};
use rhdl_roce::qp::mr::{MemoryRegion, MrWrite};
use rhdl_roce::qp::table::QpWrite;
use rhdl_roce::rx::endpoint::{In, RoceWriteRx};

// NOTE: the corpus path goes through `CARGO_MANIFEST_DIR` rather than
// being written relative to this file. This example is also
// `include_str!`-ed into a module's rustdoc and compiled as a doctest,
// and a relative `include_str!` inside it then resolves against *that
// module's* directory, not this one -- so `../tests/...` works from
// `examples/` and from `src/tx.rs`, but breaks from `src/cm/node.rs`.
// An absolute path resolves the same way from everywhere.
/// A frame from the committed Scapy corpus, by vector name.
fn vector_frame(name: &str) -> Vec<u8> {
    let corpus = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/vectors/write_vectors.txt"
    ));
    for record in corpus.split("\n\n") {
        let mut found = false;
        for line in record.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let (k, v) = (k.trim(), v.trim());
                if k == "name" && v == name {
                    found = true;
                }
                if found && k == "frame" {
                    return (0..v.len() / 2)
                        .map(|i| u8::from_str_radix(&v[2 * i..2 * i + 2], 16).unwrap())
                        .collect();
                }
            }
        }
    }
    panic!("vector {name} not found");
}

fn main() -> Result<(), RHDLError> {
    let doc = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("doc");
    std::fs::create_dir_all(&doc).expect("creating doc/");

    let idle = In {
        data: None,
        start_of_frame: false,
        end_of_frame: false,
        mr_write: None,
        qp_write: None,
    };

    // Register a region covering the message, mapped to local 0x20000.
    let mut stream = vec![
        In {
            mr_write: Some(MrWrite {
                rkey: bits(0x0000_A5A5),
                region: Some(MemoryRegion {
                    rkey: bits(0x0000_A5A5),
                    va_base: bits(0x0000_7F00_1234_0000),
                    length: bits(4096),
                    local_base: bits(0x0002_0000),
                    remote_write: true,
                }),
            }),
            ..idle
        },
        In {
            qp_write: Some(QpWrite {
                qpn: bits(0x000111),
                ctx: Some(QpContext {
                    dqpn: bits(0x000111),
                    transport: Transport::Uc,
                    pmtu: Pmtu::B1024,
                    rkey: bits(0x0000_A5A5),
                    pkey: bits(0xFFFF),
                }),
            }),
            ..idle
        },
        idle,
        idle,
    ];

    let frame = vector_frame("uc_write_only_16");
    let last = frame.len() - 1;
    for (k, b) in frame.iter().enumerate() {
        stream.push(In {
            data: Some(bits(*b as u128)),
            start_of_frame: k == 0,
            end_of_frame: k == last,
            ..idle
        });
    }
    stream.extend(std::iter::repeat_n(idle, 4));

    let uut: RoceWriteRx<4, 4> = RoceWriteRx::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(input).collect::<SvgFile>();
    let markdown = render_svg_markdown(
        vcd,
        SvgOptions::default().with_io_filter().with_label_width(26),
    )
    .expect("rendering the waveform");
    std::fs::write(doc.join("rx_endpoint.md"), markdown).expect("writing doc/rx_endpoint.md");
    Ok(())
}
