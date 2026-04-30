use rhdl::prelude::*;
use rhdl_fpga::{
    doc::{write_fsm_diagram, write_svg_as_markdown},
    serial_bus::modbus_rtu_master::{In, ModbusRtuMaster},
};

fn main() -> Result<(), RHDLError> {
    write_fsm_diagram::<ModbusRtuMaster<8, 8>>("modbus_rtu_master_fsm.md")?;

    let uut: ModbusRtuMaster<8, 8> = ModbusRtuMaster::default();

    fn idle_in() -> In<8, 8> {
        In {
            slave_addr: bits(0),
            fc: bits(0),
            addr: bits(0),
            count_or_value: bits(0),
            write_regs: [bits(0); 8],
            write_coils: [false; 8],
            start: false,
            rx_byte: bits(0),
            rx_valid: false,
            tx_ready: false,
        }
    }

    // Compute the canonical FC 0x03 response: 5 holding registers,
    // all zero.  Frame: 01 03 0A 00 00 00 00 00 00 00 00 00 00 + crc.
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

    let mut response = vec![0x01, 0x03, 0x0A];
    for _ in 0..10 {
        response.push(0);
    }
    let crc = ref_crc(&response);
    response.push((crc & 0xFF) as u8);
    response.push((crc >> 8) as u8);

    let mut stream_in: Vec<In<8, 8>> = Vec::new();
    // Request: read 5 holding registers from slave 1 starting at addr 0.
    stream_in.push(In {
        slave_addr: bits(0x01),
        fc: bits(0x03),
        addr: bits(0x0000),
        count_or_value: bits(5),
        start: true,
        ..idle_in()
    });
    for _ in 0..200 {
        stream_in.push(idle_in());
    }
    for _ in 0..32 {
        stream_in.push(In {
            tx_ready: true,
            ..idle_in()
        });
        stream_in.push(idle_in());
    }
    for &b in &response {
        stream_in.push(In {
            rx_byte: bits(b as u128),
            rx_valid: true,
            ..idle_in()
        });
        stream_in.push(idle_in());
    }
    for _ in 0..400 {
        stream_in.push(idle_in());
    }

    let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.*)|(^top.output.*)|(^top.clock.*)|(^top.reset.*)")
        .with_label_width(20);
    write_svg_as_markdown(vcd, "modbus_rtu_master.md", options)?;
    Ok(())
}
