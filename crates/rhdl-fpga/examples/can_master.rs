use rhdl::prelude::*;
use rhdl_fpga::{
    doc::{write_fsm_diagram, write_svg_as_markdown},
    serial_bus::can_master::{CanMaster, In},
};

fn main() -> Result<(), RHDLError> {
    write_fsm_diagram::<CanMaster<5>>("can_master_fsm.md")?;

    let uut = CanMaster::<5>::new(bits(4));
    let idle = In {
        rx: true,
        tx_id: bits(0),
        tx_extended: false,
        tx_rtr: false,
        tx_dlc: bits(0),
        tx_data: bits(0),
        tx_request: false,
        acc_id_filter: bits(0),
        acc_id_mask: bits(0),
    };
    let mut req = idle;
    req.tx_request = true;
    req.tx_id = bits(0x123);
    req.tx_dlc = bits(1);
    req.tx_data = bits(0xA5_00_00_00_00_00_00_00);

    let mut stream_in: Vec<In> = vec![req];
    for _ in 0..400 {
        stream_in.push(idle);
    }
    let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.*)|(^top.output.*)|(^top.clock.*)|(^top.reset.*)")
        .with_label_width(20);
    write_svg_as_markdown(vcd, "can_master.md", options)?;
    Ok(())
}
