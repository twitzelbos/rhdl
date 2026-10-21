// Phase 4 in one picture: negotiate a connection over UDP, then
// receive an RDMA WRITE into the memory it named.
//
// Two frames arrive on one byte stream.  The first is a
// connection-manager Connect datagram; the second is a RoCEv2
// WRITE_ONLY from the committed Scapy corpus, so its ICRC is real.
// Nothing is configured by hand.
//
// What to look for:
//
//   - during the first frame, `cm_phase` walks through the request and
//     `cm_status` becomes `Accepted`.  `reply` then streams 82 bytes
//     back to whoever asked.
//   - the receiver sees that same datagram and reports
//     `Frame(NotRocePort)` at its `done`.  That is not a fault: both
//     children see every frame, and each rejects what is not addressed
//     to its UDP port.  Those two independent rejections are what make
//     the fan-out work without a third parser to demultiplex.
//   - during the second frame, `write` carries the payload to addresses
//     derived from the `local_base` the *datagram* supplied -- which is
//     the whole point.  `outcome` is `Placed`.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::render_svg_markdown;
use rhdl_roce::cm::message::CM_UDP_PORT;
use rhdl_roce::cm::node::{In, RoceUcRxNode};
use rhdl_roce::frame::FrameConfig;

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

/// A Connect request for queue pair 0x111 and the region the corpus
/// vector writes to. Assembled by hand, per `cm::message`'s layout.
fn connect_request() -> Vec<u8> {
    let our_mac: u64 = 0x000a_3501_0203;
    let peer_mac: u64 = 0xec0d_9aaa_bbcc;
    let our_ip: u32 = 0xC0A8_010A;
    let peer_ip: u32 = 0xC0A8_0132;
    let mut f: Vec<u8> = Vec::new();
    f.extend_from_slice(&our_mac.to_be_bytes()[2..]);
    f.extend_from_slice(&peer_mac.to_be_bytes()[2..]);
    f.extend_from_slice(&0x0800u16.to_be_bytes());
    f.push(0x45);
    f.push(0x00);
    f.extend_from_slice(&68u16.to_be_bytes());
    f.extend_from_slice(&0u16.to_be_bytes());
    f.extend_from_slice(&0u16.to_be_bytes());
    f.push(64);
    f.push(17);
    f.extend_from_slice(&0u16.to_be_bytes());
    f.extend_from_slice(&peer_ip.to_be_bytes());
    f.extend_from_slice(&our_ip.to_be_bytes());
    f.extend_from_slice(&0x9ABCu16.to_be_bytes());
    f.extend_from_slice(&(CM_UDP_PORT as u16).to_be_bytes());
    f.extend_from_slice(&48u16.to_be_bytes());
    f.extend_from_slice(&0u16.to_be_bytes());
    f.extend_from_slice(b"RCM1");
    f.push(1); // Connect
    f.push(0);
    f.push(1); // UC
    f.push(3); // MTU 1024
    f.extend_from_slice(&0x000111u32.to_be_bytes()[1..]);
    f.push(0);
    f.extend_from_slice(&0x000222u32.to_be_bytes()[1..]);
    f.push(0);
    f.extend_from_slice(&1u32.to_be_bytes()[1..]);
    f.push(0);
    f.extend_from_slice(&0x0000_A5A5u32.to_be_bytes());
    f.extend_from_slice(&0x0000_7F00_1234_0000u64.to_be_bytes());
    f.extend_from_slice(&4096u32.to_be_bytes());
    f.extend_from_slice(&0x0002_0000u32.to_be_bytes());
    f
}

fn main() -> Result<(), RHDLError> {
    let doc = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("doc");
    std::fs::create_dir_all(&doc).expect("creating doc/");

    let idle = In {
        data: None,
        start_of_frame: false,
        end_of_frame: false,
        cfg: FrameConfig {
            dst_mac: bits(0),
            src_mac: bits(0x000a_3501_0203),
            src_ip: bits(0xC0A8_010A),
            dst_ip: bits(0),
            src_port: bits(0),
            dst_port: bits(0),
            ip_id: bits(0),
            ttl: bits(64),
            dscp_ecn: bits(0),
        },
        cm_port: bits(CM_UDP_PORT),
        reply_ready: true,
        qp_write: None,
        mr_write: None,
    };

    let mut stream = vec![idle];
    for frame in [connect_request(), vector_frame("uc_write_only_16")] {
        let last = frame.len() - 1;
        for (k, b) in frame.iter().enumerate() {
            stream.push(In {
                data: Some(bits(*b as u128)),
                start_of_frame: k == 0,
                end_of_frame: k == last,
                ..idle
            });
        }
        // Long enough for the 82-byte reply to drain, as a host waiting
        // for its acknowledgement would be.
        stream.extend(std::iter::repeat_n(idle, 95));
    }

    let uut: RoceUcRxNode<4, 4> = RoceUcRxNode::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(input).collect::<SvgFile>();
    let markdown = render_svg_markdown(
        vcd,
        SvgOptions::default().with_io_filter().with_label_width(26),
    )
    .expect("rendering the waveform");
    std::fs::write(doc.join("cm_node.md"), markdown).expect("writing doc/cm_node.md");
    Ok(())
}
