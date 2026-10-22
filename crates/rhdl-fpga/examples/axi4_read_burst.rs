use rhdl::prelude::*;
use rhdl_fpga::axi4::manager::read_burst::{Axi4ReadBurst, In};
use rhdl_fpga::axi4::types::{AxiAddress, AxiReadBeat, AxiResponse, BurstKind};
use rhdl_fpga::doc::{write_fsm_diagram, write_svg_as_markdown};

type Burst = Axi4ReadBurst<32, 32, 4, 4>;

fn main() -> Result<(), RHDLError> {
    // One four-beat INCR read from an always-ready subordinate that
    // returns beats back-to-back, with RLAST on the fourth.
    let cmd = AxiAddress::<32, 4> {
        id: bits(1),
        addr: bits(0x2000),
        len: bits(3),
        size: bits(2),
        burst: BurstKind::Incr,
        ..Default::default()
    };
    let input: Vec<In<32, 32, 4, 4>> = (0..12)
        .map(|c| In::<32, 32, 4, 4> {
            cmd: if c == 0 { Some(cmd) } else { None },
            ar_ready: true,
            // Beats arrive on cycles 2..=5; the fourth carries RLAST.
            r_valid: (2..=5).contains(&c),
            r: AxiReadBeat {
                id: bits(1),
                data: bits(0xD0 + c as u128),
                resp: AxiResponse::Okay,
                last: c == 5,
            },
            sink_ready: true,
        })
        .collect();
    let input = input.into_iter().with_reset(1).clock_pos_edge(100);
    let uut = Burst::default();
    let vcd = uut.run(input).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.cmd)|(^top.input.r_valid)|(^top.input.r.last)|(^top.output.ar_valid)|(^top.output.data)|(^top.output.busy)|(^top.output.done)")
        .with_label_width(22);
    write_svg_as_markdown(vcd, "axi4_read_burst.md", options)?;
    write_fsm_diagram::<Burst>("axi4_read_burst_fsm.md")?;
    Ok(())
}
