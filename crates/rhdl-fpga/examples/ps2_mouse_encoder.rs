use rhdl::prelude::*;
use rhdl_fpga::{
    doc::{write_fsm_diagram, write_svg_as_markdown},
    serial_bus::ps2_mouse_encoder::{In, Ps2MouseEncoder},
};
fn main() -> Result<(), RHDLError> {
    write_fsm_diagram::<Ps2MouseEncoder<8>>("ps2_mouse_encoder_fsm.md")?;
    let idle = In {
        dx: signed::<9>(0),
        dy: signed::<9>(0),
        dz: signed::<4>(0),
        btn_left: false,
        btn_right: false,
        btn_middle: false,
        btn_4: false,
        btn_5: false,
        send: false,
        clk_in: true,
    };
    let mut stream_in: Vec<In> = vec![idle; 2];
    let mut s = idle;
    s.dx = signed::<9>(7);
    s.dy = signed::<9>(-2);
    s.dz = signed::<4>(1);
    s.btn_middle = true;
    s.send = true;
    stream_in.push(s);
    for _ in 0..400 {
        stream_in.push(idle);
    }
    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let uut = Ps2MouseEncoder::<8>::new(bits(4));
    let svg = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(svg, "ps2_mouse_encoder.md", SvgOptions::default())?;
    Ok(())
}
