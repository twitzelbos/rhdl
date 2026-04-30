use rhdl::prelude::*;
use rhdl_fpga::{
    doc::{write_fsm_diagram, write_svg_as_markdown},
    serial_bus::ps2_host_tx::{In, Ps2HostTx},
};

fn main() -> Result<(), RHDLError> {
    write_fsm_diagram::<Ps2HostTx<8>>("ps2_host_tx_fsm.md")?;

    let mut stream_in: Vec<In> = vec![
        In { tx_byte: bits(0), tx_strobe: false, clk_in: true, data_in: true };
        4
    ];
    let mut start = In { tx_byte: bits(0xED), tx_strobe: true, clk_in: true, data_in: true };
    stream_in.push(start);
    start.tx_strobe = false;
    for _ in 0..16 {
        stream_in.push(start);
    }
    // Simulate device clocking + ack on last bit.
    for cycle in 0..11 {
        for _ in 0..4 {
            stream_in.push(In {
                tx_byte: bits(0xED), tx_strobe: false,
                clk_in: true,
                data_in: true,
            });
        }
        for _ in 0..4 {
            stream_in.push(In {
                tx_byte: bits(0xED), tx_strobe: false,
                clk_in: false,
                data_in: if cycle == 10 { false } else { true },
            });
        }
    }
    for _ in 0..6 {
        stream_in.push(In {
            tx_byte: bits(0), tx_strobe: false, clk_in: true, data_in: true,
        });
    }

    let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
    let uut = Ps2HostTx::<8>::new(bits(4));
    let svg = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(svg, "ps2_host_tx.md", SvgOptions::default())?;
    Ok(())
}
