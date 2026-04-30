use rhdl::prelude::*;
use rhdl_fpga::{
    doc::{write_fsm_diagram, write_svg_as_markdown},
    serial_bus::modbus_rtu_slave::{In, ModbusRtuSlave},
};

fn main() -> Result<(), RHDLError> {
    write_fsm_diagram::<ModbusRtuSlave<8, 8>>("modbus_rtu_slave_fsm.md")?;

    fn ref_crc(payload: &[u8]) -> u16 {
        let mut crc: u16 = 0xFFFF;
        for &b in payload {
            crc ^= b as u16;
            for _ in 0..8 {
                if crc & 1 == 1 {
                    crc = (crc >> 1) ^ 0xA001;
                } else {
                    crc >>= 1;
                }
            }
        }
        crc
    }

    // Send FC 0x06 write reg[2] = 0xCAFE, then drain TX, then idle.
    let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
    let mut frame = vec![0x01u8, 0x06, 0x00, 0x02, 0xCA, 0xFE];
    let crc = ref_crc(&frame);
    frame.push((crc & 0xFF) as u8);
    frame.push((crc >> 8) as u8);

    let mut stream_in: Vec<In<8, 8>> = Vec::new();
    let idle = In {
        rx_byte: bits(0),
        rx_valid: false,
        tx_ready: false,
        input_regs: [bits(0); 8],
        discrete_inputs: [false; 8],
    };
    for &b in &frame {
        stream_in.push(In {
            rx_byte: bits(b as u128),
            rx_valid: true,
            ..idle
        });
        stream_in.push(idle);
    }
    for _ in 0..150 {
        stream_in.push(idle);
    }
    for _ in 0..16 {
        stream_in.push(In {
            tx_ready: true,
            ..idle
        });
        stream_in.push(idle);
    }

    let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.*)|(^top.output.*)|(^top.clock.*)|(^top.reset.*)")
        .with_label_width(20);
    write_svg_as_markdown(vcd, "modbus_rtu_slave.md", options)?;
    Ok(())
}
