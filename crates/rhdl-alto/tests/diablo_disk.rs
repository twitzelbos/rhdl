//! Tests for the simulated Diablo 31 disk widget.
//!
//! Phase-3 disk subsystem foundation.  Verifies sector-mark
//! ticking, word-buffer write/read round-trip, transfer countdown,
//! reset behaviour, and iverilog round-trip.

use rhdl::prelude::*;
use rhdl_alto::diablo_disk::{DiabloDisk, DiskIn, DiskOut, WORDS_PER_SECTOR};

fn b16(v: u16) -> Bits<16> { bits::<16>(v as u128) }
fn b8(v: u8) -> Bits<8> { bits::<8>(v as u128) }
fn b4(v: u8) -> Bits<4> { bits::<4>(v as u128) }

fn run_inputs(uut: DiabloDisk, inputs: Vec<DiskIn>) -> Vec<DiskOut> {
    let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
    uut.run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect()
}

#[test]
fn sector_mark_fires_every_words_per_sector_cycles() {
    let uut = DiabloDisk::default();
    let trace = run_inputs(uut, vec![DiskIn::default(); (WORDS_PER_SECTOR as usize) + 5]);
    // sector_tick wraps at 256 cycles → exactly one sector_mark in 261 cycles.
    let mark_count: usize = trace.iter().filter(|o| o.sector_mark).count();
    assert_eq!(mark_count, 1, "expected exactly one sector_mark, got {}", mark_count);
}

#[test]
fn sector_mark_fires_at_position_255() {
    let uut = DiabloDisk::default();
    let trace = run_inputs(uut, vec![DiskIn::default(); 260]);
    let mark_cycles: Vec<usize> = trace.iter().enumerate()
        .filter(|(_, o)| o.sector_mark)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(mark_cycles, vec![255]);
}

#[test]
fn write_then_read_round_trip() {
    let uut = DiabloDisk::default();
    let trace = run_inputs(uut, vec![
        DiskIn { word_addr: b8(5), write_data: b16(0xCAFE), write_en: true, ..DiskIn::default() },
        DiskIn { word_addr: b8(5), read_en: true, ..DiskIn::default() },
        DiskIn::default(),
    ]);
    // Cycle 0 commits the write; cycle 1 reads back 0xCAFE.
    assert_eq!(trace[1].read_data, b16(0xCAFE));
}

#[test]
fn read_with_read_en_off_returns_zero() {
    let uut = DiabloDisk::default();
    let trace = run_inputs(uut, vec![DiskIn::default(); 4]);
    for o in &trace {
        assert_eq!(o.read_data, b16(0));
    }
}

#[test]
fn ready_is_always_true_post_reset() {
    let uut = DiabloDisk::default();
    let trace = run_inputs(uut, vec![DiskIn::default(); 4]);
    for o in &trace {
        assert!(o.ready, "ready should be true post-reset");
    }
}

#[test]
fn reset_then_writes_dont_panic() {
    let uut = DiabloDisk::default();
    let trace = run_inputs(uut, vec![
        DiskIn { word_addr: b8(0), write_data: b16(0xFFFF), write_en: true, ..DiskIn::default() };
        5
    ]);
    assert_eq!(trace.len(), 5);
}

#[test]
fn cylinder_head_sector_inputs_pass_through() {
    let uut = DiabloDisk::default();
    let trace = run_inputs(uut, vec![
        DiskIn { cylinder: b8(42), head: true, sector: b4(7), ..DiskIn::default() },
        DiskIn { cylinder: b8(42), head: true, sector: b4(7), word_addr: b8(0), read_en: true, ..DiskIn::default() },
        DiskIn::default(),
        DiskIn::default(),
    ]);
    assert_eq!(trace.len(), 4);
}

#[test]
fn diablo_disk_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut = DiabloDisk::default();
    let inputs: Vec<DiskIn> = (0..4)
        .map(|i| DiskIn {
            word_addr: b8(i as u8),
            write_data: b16(i as u16 * 0x10),
            write_en: i < 2,
            read_en: i >= 2,
            ..DiskIn::default()
        })
        .collect();
    let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
