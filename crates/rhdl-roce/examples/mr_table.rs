// Memory-region registration, then four accesses against it.
//
// The region covers 4 KiB of a peer's address space starting at
// 0x7F00_1234_0000 and lands locally at 0x0001_0000.  What to look for,
// in order:
//
//   - a write registers it; `region` reflects the slot one cycle later,
//     because the table is a synchronous RAM.
//   - a legitimate access in the middle of the region is accepted and
//     translated.
//   - an access with the right *index* but the wrong R_Key is rejected
//     `RkeyMismatch`.  That is the check that makes indexing by the
//     key's low bits safe; without it, any key sharing those bits would
//     be granted this region.
//   - an access that starts inside the region and runs off the end is
//     rejected `OutOfRange`.  Checking only the start address would
//     have let it through.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::render_svg_markdown;
use rhdl_roce::qp::mr::{In, MemoryRegion, MrTable, MrWrite};

const RKEY: u128 = 0x0000_A5A5;
const VA: u128 = 0x0000_7F00_1234_0000;
const LEN: u128 = 4096;

fn main() -> Result<(), RHDLError> {
    let doc = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("doc");
    std::fs::create_dir_all(&doc).expect("creating doc/");

    let region = MemoryRegion {
        rkey: bits(RKEY),
        va_base: bits(VA),
        length: bits(LEN),
        local_base: bits(0x0001_0000),
        remote_write: true,
    };
    let idle = In {
        read_rkey: bits(0),
        write: None,
    };

    let mut stream = vec![
        // Register the region.
        In {
            read_rkey: bits(RKEY),
            write: Some(MrWrite {
                rkey: bits(RKEY),
                region: Some(region),
            }),
        },
        // Read it back.
        In {
            read_rkey: bits(RKEY),
            ..idle
        },
        // A different R_Key sharing the low bits -- same slot.
        In {
            read_rkey: bits(0x1000_A5A5),
            ..idle
        },
        In {
            read_rkey: bits(RKEY),
            ..idle
        },
    ];
    stream.extend(std::iter::repeat_n(idle, 4));

    let uut: MrTable<4> = MrTable::default();
    let input = stream.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(input).collect::<SvgFile>();
    let markdown = render_svg_markdown(
        vcd,
        SvgOptions::default().with_io_filter().with_label_width(26),
    )
    .expect("rendering the waveform");
    std::fs::write(doc.join("mr_table.md"), markdown).expect("writing doc/mr_table.md");
    Ok(())
}
