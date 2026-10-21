// A queue pair negotiated over UDP.
//
// One Connect datagram arrives and is answered.  The request is
// assembled here from explicit big-endian pushes against the layout
// table in `cm::message`'s docs, rather than with the crate's own
// encoder -- which would prove only that the encoder and decoder agree.
//
// What to look for:
//
//   - `qp_write` and `mr_write` are asserted together, for exactly one
//     cycle, at the end of the request.  Together because a half-applied
//     configuration -- a queue pair with no region, or the reverse -- is
//     not a state anything should observe.
//   - `status` becomes `Accepted` in the same cycle.
//   - `reply` then streams 82 bytes: Ethernet, IPv4, UDP and the 40-byte
//     response payload.  Its destination is taken from the request's
//     source fields, so nothing about the peer is configured anywhere.
//   - `phase` walks Idle -> Header -> Body -> Reply -> Idle.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::{render_fsm_diagram_markdown, render_svg_markdown};
use rhdl_roce::cm::manager::{ConnectionManager, In};
use rhdl_roce::cm::message::CM_UDP_PORT;
use rhdl_roce::frame::FrameConfig;

/// A Connect request, byte by byte, per the layout in `cm::message`.
fn connect_request() -> Vec<u8> {
    let our_mac: u64 = 0x000a_3501_0203;
    let peer_mac: u64 = 0xec0d_9aaa_bbcc;
    let our_ip: u32 = 0xC0A8_0132;
    let peer_ip: u32 = 0xC0A8_010A;
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
    f.push(0); // status, zero in a request
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

    let transitions = rhdl::core::fsm::extract_widget_transitions_strict::<ConnectionManager>()
        .expect("extracting the FSM transitions");
    std::fs::write(
        doc.join("cm_manager_fsm.md"),
        render_fsm_diagram_markdown::<ConnectionManager>(&transitions),
    )
    .expect("writing doc/cm_manager_fsm.md");

    let idle = In {
        data: None,
        start_of_frame: false,
        end_of_frame: false,
        cfg: FrameConfig {
            // The destination fields are overwritten from the request,
            // so what is here is immaterial.
            dst_mac: bits(0),
            src_mac: bits(0x000a_3501_0203),
            src_ip: bits(0xC0A8_0132),
            dst_ip: bits(0),
            src_port: bits(0),
            dst_port: bits(0),
            ip_id: bits(0),
            ttl: bits(64),
            dscp_ecn: bits(0),
        },
        port: bits(CM_UDP_PORT),
        ready: true,
    };

    let req = connect_request();
    let last = req.len() - 1;
    let mut stream = vec![idle];
    for (k, b) in req.iter().enumerate() {
        stream.push(In {
            data: Some(bits(*b as u128)),
            start_of_frame: k == 0,
            end_of_frame: k == last,
            ..idle
        });
    }
    // Enough cycles for the 82-byte reply to go out.
    stream.extend(std::iter::repeat_n(idle, 90));

    let uut = ConnectionManager::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(input).collect::<SvgFile>();
    let markdown = render_svg_markdown(
        vcd,
        SvgOptions::default().with_io_filter().with_label_width(26),
    )
    .expect("rendering the waveform");
    std::fs::write(doc.join("cm_manager.md"), markdown).expect("writing doc/cm_manager.md");
    Ok(())
}
