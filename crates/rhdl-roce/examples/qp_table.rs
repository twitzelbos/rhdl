// Three queue pairs configured, read back, and one torn down.
//
// What to look for:
//
//   - A fresh table reads back `None` everywhere. Not a zeroed context:
//     a zeroed QpContext is a *plausible* connection to queue pair 0,
//     and transmitting to it would be worse than refusing to. Validity
//     is the Option itself, so there is no separate bitmap that can
//     disagree with the data.
//   - `ctx` lags `read_qpn` by one cycle, because the table is block
//     RAM. For the send path this costs nothing -- the queue-pair number
//     is constant for a whole message, so the lookup happens once at
//     message start.
//   - Each queue pair reads back its own context. That is the whole
//     point: until this existed the endpoint had one connection, with
//     the context passed in as an input.
//   - The last write stores `None` for queue pair 2, and the following
//     read reports `valid` low. That is how an entry is invalidated.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::render_svg_markdown;
use rhdl_roce::headers::Transport;
use rhdl_roce::qp::context::{Pmtu, QpContext};
use rhdl_roce::qp::table::{In, QpTable, QpWrite};

fn ctx(n: u128) -> QpContext {
    QpContext {
        dqpn: bits(0x1000 + n),
        transport: if n % 2 == 0 {
            Transport::Uc
        } else {
            Transport::Rc
        },
        pmtu: Pmtu::B1024,
        rkey: bits(0xA000_0000 + n),
        pkey: bits(0xFFFF),
    }
}

fn main() -> Result<(), RHDLError> {
    let idle = In {
        read_qpn: bits(0),
        write: None,
    };
    let mut stream = vec![idle];
    for (k, qpn) in [1u128, 2, 5].iter().enumerate() {
        stream.push(In {
            read_qpn: bits(0),
            write: Some(QpWrite {
                qpn: bits(*qpn),
                ctx: Some(ctx(k as u128 + 1)),
            }),
        });
    }
    for qpn in [1u128, 2, 5, 9] {
        stream.push(In {
            read_qpn: bits(qpn),
            write: None,
        });
    }
    // Tear down queue pair 2, then read it back.
    stream.push(In {
        read_qpn: bits(0),
        write: Some(QpWrite {
            qpn: bits(2),
            ctx: None,
        }),
    });
    stream.push(In {
        read_qpn: bits(2),
        write: None,
    });
    stream.push(idle);
    stream.push(idle);

    let uut = QpTable::<4>::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(input).collect::<SvgFile>();
    let markdown = render_svg_markdown(
        vcd,
        SvgOptions::default().with_io_filter().with_label_width(22),
    )
    .expect("rendering the waveform");
    let doc = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("doc");
    std::fs::create_dir_all(&doc).expect("creating doc/");
    std::fs::write(doc.join("qp_table.md"), markdown).expect("writing doc/qp_table.md");
    Ok(())
}
