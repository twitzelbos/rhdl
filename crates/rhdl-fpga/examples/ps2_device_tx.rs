use rhdl::prelude::*;
use rhdl_fpga::{
    doc::{write_fsm_diagram, write_svg_as_markdown},
    serial_bus::ps2_device_tx::{In, Ps2DeviceTx},
};

fn main() -> Result<(), RHDLError> {
    write_fsm_diagram::<Ps2DeviceTx<8>>("ps2_device_tx_fsm.md")?;
    let mut stream_in: Vec<In> = vec![In { tx_byte: bits(0), tx_strobe: false, clk_in: true }; 2];
    stream_in.push(In { tx_byte: bits(0x55), tx_strobe: true, clk_in: true });
    for _ in 0..200 {
        stream_in.push(In { tx_byte: bits(0), tx_strobe: false, clk_in: true });
    }
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let uut = Ps2DeviceTx::<8>::new(bits(4));
    let svg = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(svg, "ps2_device_tx.md", SvgOptions::default())?;
    Ok(())
}
