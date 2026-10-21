// A UDP datagram taken apart, byte by byte.
//
// Two datagrams arrive on one stream.  The first is addressed to the
// port we are listening on; the second is identical except for its
// destination port.  Both are assembled here from explicit big-endian
// pushes, so nothing in the picture is this widget's own idea of what a
// header looks like.
//
// What to look for:
//
//   - `phase` walks Idle -> Network -> Payload -> Idle on the first
//     datagram.  `header` appears in the same cycle as the first
//     payload byte -- the earliest it could, since the last header byte
//     only reaches the registers at the end of the cycle before.
//   - `payload_index` counts from zero at the payload, while
//     `frame_index` keeps counting from the start of the frame.  Both
//     are emitted because a protocol whose integrity check covers the
//     frame from the IP header onward needs the second, and its own
//     fields need the first.
//   - `byte` carries *every* byte, header bytes included.  That is the
//     pass-through design: a filtering parser would make its caller
//     keep a second copy of the stream to recover them.
//   - on the second datagram the port check at offset 37 fails, `phase`
//     goes to Drain, and no payload is emitted at all.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::{write_fsm_diagram, write_svg_as_markdown};
use rhdl_fpga::net::udp_rx::{In, UdpRx};

const OUR_PORT: u16 = 4792;

/// An Ethernet/IPv4/UDP datagram with `payload` bytes, built by hand.
fn datagram(dst_port: u16, payload: &[u8]) -> Vec<u8> {
    let mut f: Vec<u8> = Vec::new();
    // Ethernet II
    f.extend_from_slice(&[0x00, 0x0a, 0x35, 0x01, 0x02, 0x03]); // destination
    f.extend_from_slice(&[0xec, 0x0d, 0x9a, 0xaa, 0xbb, 0xcc]); // source
    f.extend_from_slice(&0x0800u16.to_be_bytes());
    // IPv4, 20 bytes, no options
    f.push(0x45);
    f.push(0x00);
    f.extend_from_slice(&((20 + 8 + payload.len()) as u16).to_be_bytes());
    f.extend_from_slice(&0u16.to_be_bytes()); // identification
    f.extend_from_slice(&0u16.to_be_bytes()); // flags / fragment
    f.push(64); // TTL
    f.push(17); // UDP
    f.extend_from_slice(&0u16.to_be_bytes()); // checksum, not verified
    f.extend_from_slice(&[192, 168, 1, 10]); // source
    f.extend_from_slice(&[192, 168, 1, 50]); // destination
    // UDP, 8 bytes
    f.extend_from_slice(&0x9abcu16.to_be_bytes()); // source port
    f.extend_from_slice(&dst_port.to_be_bytes());
    f.extend_from_slice(&((8 + payload.len()) as u16).to_be_bytes());
    f.extend_from_slice(&0u16.to_be_bytes()); // checksum
    f.extend_from_slice(payload);
    f
}

fn main() -> Result<(), RHDLError> {
    write_fsm_diagram::<UdpRx>("udp_rx_fsm.md").expect("writing the FSM diagram");

    let idle = In {
        data: None,
        start_of_frame: false,
        end_of_frame: false,
        expect_port: bits(OUR_PORT as u128),
    };
    let payload: Vec<u8> = (0..8u8).map(|k| 0xA0 | k).collect();

    let mut stream = vec![idle];
    for port in [OUR_PORT, OUR_PORT + 1] {
        let f = datagram(port, &payload);
        let last = f.len() - 1;
        for (k, b) in f.iter().enumerate() {
            stream.push(In {
                data: Some(bits(*b as u128)),
                start_of_frame: k == 0,
                end_of_frame: k == last,
                ..idle
            });
        }
        stream.push(idle);
        stream.push(idle);
    }

    let uut = UdpRx::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(input).collect::<SvgFile>();
    write_svg_as_markdown(
        vcd,
        "udp_rx.md",
        SvgOptions::default().with_io_filter().with_label_width(26),
    )
    .expect("writing the waveform");
    Ok(())
}
