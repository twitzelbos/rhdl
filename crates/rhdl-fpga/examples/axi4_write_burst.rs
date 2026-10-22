use rhdl::prelude::*;
use rhdl_fpga::axi4::manager::write_burst::{Axi4WriteBurst, In};
use rhdl_fpga::axi4::types::{AxiAddress, AxiResponse, AxiWriteResponse, BurstKind};
use rhdl_fpga::doc::{write_fsm_diagram, write_svg_as_markdown};

// A 32-bit data bus: four byte lanes, so STRB is 4.
type Burst = Axi4WriteBurst<32, 32, 4, 4>;

fn main() -> Result<(), RHDLError> {
    // One four-beat INCR burst, with an always-ready subordinate that
    // answers OKAY. The trace shows the sequencer walking
    // Idle -> Addr -> Data -> Resp and back.
    let cmd = AxiAddress::<32, 4> {
        id: bits(1),
        addr: bits(0x1000),
        len: bits(3),
        size: bits(2),
        burst: BurstKind::Incr,
        ..Default::default()
    };
    let mut input = Vec::new();
    for cycle in 0..12 {
        input.push(In::<32, 32, 4> {
            cmd: if cycle == 0 { Some(cmd) } else { None },
            data: Some(bits(0xA0 + cycle as u128)),
            aw_ready: true,
            w_ready: true,
            // The subordinate answers once the data phase can have ended.
            b_valid: cycle >= 6,
            b: AxiWriteResponse {
                id: bits(1),
                resp: AxiResponse::Okay,
            },
        });
    }
    let input = input.into_iter().with_reset(1).clock_pos_edge(100);
    let uut = Burst::default();
    let vcd = uut.run(input).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.cmd)|(^top.output.aw_valid)|(^top.output.w_valid)|(^top.output.w.last)|(^top.output.busy)|(^top.output.done)")
        .with_label_width(22);
    write_svg_as_markdown(vcd, "axi4_write_burst.md", options)?;
    write_fsm_diagram::<Burst>("axi4_write_burst_fsm.md")?;
    Ok(())
}
