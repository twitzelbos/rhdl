use rhdl::prelude::*;
use rhdl_fpga::{
    doc::{write_fsm_diagram, write_svg_as_markdown},
    serial_bus::can_receiver::{CanReceiver, In},
};

fn main() -> Result<(), RHDLError> {
    // FSM diagram first per CLAUDE.md §12 rule 14.
    write_fsm_diagram::<CanReceiver<5>>("can_receiver_fsm.md")?;

    // Drive the receiver with a synthetic SOF + idle waveform.
    // Round-trip validation against can_master lives in
    // can_master::tests::test_two_node_*.
    let bit_period = 4u128;
    let mut stream_in: Vec<In> = vec![
        In {
            rx: true,
            drive_ack: false
        };
        4
    ];
    stream_in.push(In {
        rx: false,
        drive_ack: false,
    });
    for _ in 0..400 {
        stream_in.push(In {
            rx: true,
            drive_ack: false,
        });
    }
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let rx_uut: CanReceiver<5> = CanReceiver::new(bits(bit_period));
    let svg = rx_uut.run(stream).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.*)|(^top.output.*)|(^top.clock.*)|(^top.reset.*)")
        .with_label_width(20);
    write_svg_as_markdown(svg, "can_receiver.md", options)?;
    Ok(())
}
