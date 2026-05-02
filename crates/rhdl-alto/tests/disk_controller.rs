//! Tests for the disk controller register file.

use rhdl::prelude::*;
use rhdl_alto::disk_controller::*;

fn b16(v: u16) -> Bits<16> { bits::<16>(v as u128) }
fn b3(v: u8) -> Bits<3> { bits::<3>(v as u128) }

fn run_inputs(uut: DiskController, inputs: Vec<CtrlIn>) -> Vec<CtrlOut> {
    let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
    uut.run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect()
}

#[test]
fn write_and_read_kstat() {
    let uut = DiskController::default();
    let trace = run_inputs(uut, vec![
        CtrlIn { reg_addr: b3(REG_KSTAT as u8), write_data: b16(0x4321), write_en: true },
        CtrlIn { reg_addr: b3(REG_KSTAT as u8), write_data: b16(0), write_en: false },
        CtrlIn { reg_addr: b3(REG_KSTAT as u8), write_data: b16(0), write_en: false },
    ]);
    // Cycle 0 commits the write; cycle 1 reads back 0x4321.
    assert_eq!(trace[1].read_data, b16(0x4321));
}

#[test]
fn kadr_field_decode() {
    // KADR layout: bits[15:8] = cylinder, bit[7] = head, bits[3:0] = sector.
    let kadr_value = (0x42 << 8) | (1 << 7) | 0x05;
    let uut = DiskController::default();
    let trace = run_inputs(uut, vec![
        CtrlIn { reg_addr: b3(REG_KADR as u8), write_data: b16(kadr_value), write_en: true },
        CtrlIn { reg_addr: b3(REG_KADR as u8), write_data: b16(0), write_en: false },
        CtrlIn { reg_addr: b3(REG_KADR as u8), write_data: b16(0), write_en: false },
    ]);
    let last = &trace[2];
    assert_eq!(last.kadr_cylinder.raw(), 0x42, "cylinder");
    assert!(last.kadr_head, "head bit set");
    assert_eq!(last.kadr_sector.raw(), 0x05, "sector");
}

#[test]
fn each_register_independent() {
    let uut = DiskController::default();
    let trace = run_inputs(uut, vec![
        // Write to each register on cycles 0..6.
        CtrlIn { reg_addr: b3(REG_KSTAT as u8), write_data: b16(0x1111), write_en: true },
        CtrlIn { reg_addr: b3(REG_KDATA as u8), write_data: b16(0x2222), write_en: true },
        CtrlIn { reg_addr: b3(REG_KCOM as u8),  write_data: b16(0x3333), write_en: true },
        CtrlIn { reg_addr: b3(REG_KADR as u8),  write_data: b16(0x4444), write_en: true },
        CtrlIn { reg_addr: b3(REG_KCWA as u8),  write_data: b16(0x5555), write_en: true },
        CtrlIn { reg_addr: b3(REG_KCWD as u8),  write_data: b16(0x6666), write_en: true },
        // Read each back on cycles 6..12.
        CtrlIn { reg_addr: b3(REG_KSTAT as u8), write_data: b16(0), write_en: false },
        CtrlIn { reg_addr: b3(REG_KDATA as u8), write_data: b16(0), write_en: false },
        CtrlIn { reg_addr: b3(REG_KCOM as u8),  write_data: b16(0), write_en: false },
        CtrlIn { reg_addr: b3(REG_KADR as u8),  write_data: b16(0), write_en: false },
        CtrlIn { reg_addr: b3(REG_KCWA as u8),  write_data: b16(0), write_en: false },
        CtrlIn { reg_addr: b3(REG_KCWD as u8),  write_data: b16(0), write_en: false },
    ]);
    assert_eq!(trace[6].read_data,  b16(0x1111));
    assert_eq!(trace[7].read_data,  b16(0x2222));
    assert_eq!(trace[8].read_data,  b16(0x3333));
    assert_eq!(trace[9].read_data,  b16(0x4444));
    assert_eq!(trace[10].read_data, b16(0x5555));
    assert_eq!(trace[11].read_data, b16(0x6666));
}

#[test]
fn disk_controller_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut = DiskController::default();
    let inputs: Vec<CtrlIn> = (0..6).map(|i| CtrlIn {
        reg_addr: b3((i % 6) as u8),
        write_data: b16(i as u16 * 0x10),
        write_en: i < 3,
    }).collect();
    let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
