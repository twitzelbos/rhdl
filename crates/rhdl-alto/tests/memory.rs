//! Tests for the Phase-3.5 64KW SyncBRAM-backed memory subsystem.
//!
//! Note: SyncBRAM has a 1-cycle read latency (real BRAM, real Alto
//! MOS-DRAM both behave this way).  After presenting an address at
//! cycle T, the data appears on `read_data` at cycle T+1.  The
//! `with_reset(1)` prefix means the first sample-of-interest is at
//! trace index 1 (the cycle after a write input → 1 cycle later
//! again before the read materializes).

use rhdl::prelude::*;
use rhdl_alto::memory::{MemIn, MemOut, Memory};

fn b16(v: u16) -> Bits<16> { bits::<16>(v as u128) }

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
        MemIn { address: b16(0x1000), write_data: b16(0xABCD), write_en: true, ..Default::default() },
        MemIn { address: b16(0x1000), write_data: b16(0), write_en: false, ..Default::default() },
        MemIn { address: b16(0x1000), write_data: b16(0), write_en: false, ..Default::default() },
        MemIn::default(),
    ]);
    // Cycle 0: write committed at end of cycle.
    // Cycle 1: read addr presented; BRAM data appears at cycle 2.
    assert_eq!(trace[2].read_data, b16(0xABCD), "BRAM read latency = 1 cycle");
}

#[test]
fn writes_are_independent_per_address() {
    let uut = Memory::default();
    let trace = run_inputs(uut, vec![
        MemIn { address: b16(0x1010), write_data: b16(0x10), write_en: true, ..Default::default() },
        MemIn { address: b16(0x2020), write_data: b16(0x20), write_en: true, ..Default::default() },
        MemIn { address: b16(0x1010), write_data: b16(0), write_en: false, ..Default::default() },
        MemIn { address: b16(0x2020), write_data: b16(0), write_en: false, ..Default::default() },
        MemIn { address: b16(0x3030), write_data: b16(0), write_en: false, ..Default::default() }, // unwritten
        MemIn::default(),
        MemIn::default(),
    ]);
    assert_eq!(trace[3].read_data, b16(0x10), "0x1010 should hold 0x10");
    assert_eq!(trace[4].read_data, b16(0x20), "0x2020 should hold 0x20");
    assert_eq!(trace[5].read_data, b16(0),    "0x3030 was never written");
}

#[test]
fn full_64k_address_space() {
    // Write to top-of-memory (0xFFFF) and read back.
    let uut = Memory::default();
    let trace = run_inputs(uut, vec![
        MemIn { address: b16(0xFFFF), write_data: b16(0xDEAD), write_en: true, ..Default::default() },
        MemIn { address: b16(0xFFFF), write_data: b16(0), write_en: false, ..Default::default() },
        MemIn::default(),
        MemIn::default(),
    ]);
    assert_eq!(trace[2].read_data, b16(0xDEAD));
}

#[test]
fn preloaded_initial_contents() {
    // Phase 3.5 boot path: the boot block (256 words) is loaded into
    // memory[1..257] before the Nova emulator starts.  Verify the
    // preloading mechanism works end-to-end.
    let boot_block = vec![0o000345, 0o000354, 0o000403, 0o114366];
    let uut = Memory::with_words_at(1, &boot_block);
    let trace = run_inputs(uut, vec![
        MemIn { address: b16(1), write_data: b16(0), write_en: false, ..Default::default() },
        MemIn { address: b16(2), write_data: b16(0), write_en: false, ..Default::default() },
        MemIn { address: b16(3), write_data: b16(0), write_en: false, ..Default::default() },
        MemIn { address: b16(4), write_data: b16(0), write_en: false, ..Default::default() },
        MemIn::default(),
    ]);
    assert_eq!(trace[1].read_data.raw() as u16, 0o000345, "memory[1] = JMP-345 entrypoint");
    assert_eq!(trace[2].read_data.raw() as u16, 0o000354, "memory[2]");
    assert_eq!(trace[3].read_data.raw() as u16, 0o000403, "memory[3]");
    assert_eq!(trace[4].read_data.raw() as u16, 0o114366, "memory[4]");
}

#[test]
fn memory_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut = Memory::with_words_at(0, &[0xA000, 0xB000, 0xC000, 0xD000, 0xE000, 0xF000]);
    let inputs: Vec<MemIn> = (0..6).map(|i| MemIn {
        address: b16(i as u16),
        write_data: b16(0),
        write_en: false,
        ..Default::default()
    }).collect();
    let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &TestBenchOptions::default().skip(2))?;
    tm.run_iverilog()?;
    Ok(())
}
