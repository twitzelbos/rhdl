use rhdl::prelude::*;
use rhdl_fpga::axi4::endpoint::mem::{Axi4MemEndpoint, In};
use rhdl_fpga::axi4::types::{AxiAddress, AxiWriteBeat, BurstKind};
use rhdl_fpga::doc::write_svg_as_markdown;

// A 32-bit bus: STRB = 4, SHIFT = log2(4) = 2, and 2^8 words of store.
type Endpoint = Axi4MemEndpoint<32, 32, 4, 2, 4, 8>;
const _: () = Endpoint::check_widths();

fn main() -> Result<(), RHDLError> {
    let aw = AxiAddress::<32, 4> {
        id: bits(1),
        addr: bits(0x40),
        len: bits(3),
        size: bits(2),
        burst: BurstKind::Incr,
        ..Default::default()
    };
    // Address, then four full-width beats, then the response handshake.
    let input: Vec<In<32, 32, 4, 4>> = (0..12)
        .map(|c| In::<32, 32, 4, 4> {
            aw_valid: c == 0,
            aw,
            w_valid: (1..=4).contains(&c),
            w: AxiWriteBeat {
                data: bits(0xC0 + c as u128),
                strb: bits(0b1111),
                last: c == 4,
            },
            b_ready: true,
            ar_valid: false,
            ar: AxiAddress::default(),
            r_ready: true,
        })
        .collect();
    let input = input.into_iter().with_reset(1).clock_pos_edge(100);
    let uut = Endpoint::default();
    let vcd = uut.run(input).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.aw_valid)|(^top.input.w_valid)|(^top.input.w.last)|(^top.output.aw_ready)|(^top.output.w_ready)|(^top.output.b_valid)|(^top.output.ar_ready)")
        .with_label_width(22);
    write_svg_as_markdown(vcd, "axi4_mem_endpoint.md", options)?;
    Ok(())
}
