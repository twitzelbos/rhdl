//! Tests for the Phase-3 memory subsystem stub.

use rhdl::prelude::*;
use rhdl_alto::memory::{MemIn, MemOut, Memory};

fn b16(v: u16) -> Bits<16> { bits::<16>(v as u128) }
fn b8(v: u8) -> Bits<8> { bits::<8>(v as u128) }

fn run_inputs(uut: Memory, inputs: Vec<MemIn>) -> Vec<MemOut> {
    let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
    uut.run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect()
}

#[test]
fn write_then_read_round_trip() {
    let uut = Memory::default();
    let trace = run_inputs(uut, vec![
        MemIn { address: b8(0x10), write_data: b16(0xABCD), write_en: true, read_en: false },
        MemIn { address: b8(0x10), write_data: b16(0), write_en: false, read_en: true },
        MemIn { address: b8(0x10), write_data: b16(0), write_en: false, read_en: true },
    ]);
    // Cycle 0 commits the write; cycle 1 reads it back.
    assert_eq!(trace[1].read_data, b16(0xABCD));
}

#[test]
fn read_with_read_en_off_returns_zero() {
    let uut = Memory::default();
    let trace = run_inputs(uut, vec![MemIn::default(); 4]);
    for o in &trace {
        assert_eq!(o.read_data, b16(0));
    }
}

#[test]
fn writes_are_independent_per_address() {
    let uut = Memory::default();
    let trace = run_inputs(uut, vec![
        MemIn { address: b8(0x10), write_data: b16(0x10), write_en: true, read_en: false },
        MemIn { address: b8(0x20), write_data: b16(0x20), write_en: true, read_en: false },
        MemIn { address: b8(0x10), write_data: b16(0), write_en: false, read_en: true },
        MemIn { address: b8(0x20), write_data: b16(0), write_en: false, read_en: true },
        MemIn { address: b8(0x30), write_data: b16(0), write_en: false, read_en: true },
    ]);
    assert_eq!(trace[2].read_data, b16(0x10), "0x10 should hold 0x10");
    assert_eq!(trace[3].read_data, b16(0x20), "0x20 should hold 0x20");
    assert_eq!(trace[4].read_data, b16(0),     "0x30 was never written");
}

#[test]
fn memory_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut = Memory::default();
    let inputs: Vec<MemIn> = (0..6).map(|i| MemIn {
        address: b8(i as u8 * 4),
        write_data: b16(i as u16 * 0x100),
        write_en: i < 3,
        read_en: i >= 3,
    }).collect();
    let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
