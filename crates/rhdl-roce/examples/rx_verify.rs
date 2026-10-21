// A frame received, parsed, and its Invariant CRC checked.
//
// Two frames go in back to back.  The first is intact; the second has
// one payload byte flipped.  What to look for:
//
//   - `computed` settles three cycles before `done`, because the ICRC
//     covers everything up to the checksum field itself -- so by the
//     time the four ICRC bytes arrive there is nothing left to fold and
//     no stall is needed.
//   - on the first frame `received` and `computed` are equal and the
//     verdict is `Accept`.
//   - on the second they differ and the verdict is `IcrcMismatch`.  The
//     headers and the payload were still presented, because they had to
//     be: the corruption is only knowable once the whole frame has
//     arrived.  That is why a consumer places speculatively and commits
//     on `done`.
//
// The frame is built with the transmitter's own pure functions and a
// real ICRC taken from the committed Scapy vector, so nothing here is
// this crate's own idea of a correct checksum.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::render_svg_markdown;
use rhdl_roce::rx::verify::{In, RxVerify};

// NOTE: the corpus path goes through `CARGO_MANIFEST_DIR` rather than
// being written relative to this file. This example is also
// `include_str!`-ed into a module's rustdoc and compiled as a doctest,
// and a relative `include_str!` inside it then resolves against *that
// module's* directory, not this one -- so `../tests/...` works from
// `examples/` and from `src/tx.rs`, but breaks from `src/cm/node.rs`.
// An absolute path resolves the same way from everywhere.
/// The `uc_write_only_16` frame from `tests/vectors/write_vectors.txt`,
/// complete with the ICRC Scapy computed for it.
fn vector_frame() -> Vec<u8> {
    let corpus = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/vectors/write_vectors.txt"
    ));
    for record in corpus.split("\n\n") {
        let mut found = false;
        for line in record.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let (k, v) = (k.trim(), v.trim());
                if k == "name" && v == "uc_write_only_16" {
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
    panic!("vector uc_write_only_16 not found");
}

fn main() -> Result<(), RHDLError> {
    let doc = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("doc");
    std::fs::create_dir_all(&doc).expect("creating doc/");

    let good = vector_frame();
    // One payload byte flipped. Offset 70 is the first payload byte of
    // a WRITE_ONLY, which carries a RETH.
    let mut bad = good.clone();
    bad[70] ^= 0xFF;

    let idle = In {
        data: None,
        start_of_frame: false,
        end_of_frame: false,
    };
    let mut stream = vec![idle];
    for frame in [&good, &bad] {
        let last = frame.len() - 1;
        for (k, b) in frame.iter().enumerate() {
            stream.push(In {
                data: Some(bits(*b as u128)),
                start_of_frame: k == 0,
                end_of_frame: k == last,
            });
        }
        stream.push(idle);
    }
    stream.push(idle);

    let uut = RxVerify::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(input).collect::<SvgFile>();
    let markdown = render_svg_markdown(
        vcd,
        SvgOptions::default().with_io_filter().with_label_width(26),
    )
    .expect("rendering the waveform");
    std::fs::write(doc.join("rx_verify.md"), markdown).expect("writing doc/rx_verify.md");
    Ok(())
}
