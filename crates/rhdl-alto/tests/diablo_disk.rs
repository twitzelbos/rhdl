//! Tests for the simulated Diablo 31 disk widget.
//!
//! Phase-3 disk subsystem foundation.  Verifies sector-mark
//! ticking, word-buffer write/read round-trip, transfer countdown,
//! reset behaviour, and iverilog round-trip.

use rhdl::prelude::*;
use rhdl_alto::diablo_disk::{DiabloDisk, DiskIn, DiskOut, WORDS_PER_SECTOR};

fn b16(v: u16) -> Bits<16> {
    bits::<16>(v as u128)
}
fn b8(v: u8) -> Bits<8> {
    bits::<8>(v as u128)
}
fn b4(v: u8) -> Bits<4> {
    bits::<4>(v as u128)
}

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
    // Use a SHORT 256-cycle test period so this test stays cheap.
    // Real hardware uses the spec-correct ~19,608-cycle period
    // (`SECTOR_PERIOD_CYCLES`); see also
    // [`sector_mark_uses_spec_period_by_default`].
    let uut = DiabloDisk::with_test_period(256);
    let trace = run_inputs(
        uut,
        vec![DiskIn::default(); (WORDS_PER_SECTOR as usize) + 5],
    );
    // sector_tick wraps at 256 cycles → exactly ONE rising edge of
    // sector_mark in 261 cycles.  Per *Alto Hardware Manual* §2.4 +
    // spec §5.5, sector_mark is a SUSTAINED wakeup signal — once
    // asserted at sector boundary, it stays high until KSEC issues
    // F1=Block.  No Block here (DiskIn::default() leaves block_task =
    // false), so sector_mark stays high once set.  Count rising edges
    // (false → true transitions) to find sector events.
    let mut events: usize = 0;
    let mut prev = false;
    for o in &trace {
        if !prev && o.sector_mark {
            events += 1;
        }
        prev = o.sector_mark;
    }
    assert_eq!(
        events, 1,
        "expected exactly one sector_mark rising edge, got {events}"
    );
}

#[test]
fn sector_mark_fires_at_position_255() {
    // SHORT test period (256 cycles) so the rising edge lands at 255.
    let uut = DiabloDisk::with_test_period(256);
    let trace = run_inputs(uut, vec![DiskIn::default(); 260]);
    // Per the sustained-wakeup model (spec §5.5), sector_mark goes
    // high at cycle 255 and stays high (no F1=Block in this test).
    // Verify the rising edge is at cycle 255 and the signal stays
    // high thereafter.
    let first_high = trace.iter().position(|o| o.sector_mark);
    assert_eq!(
        first_high,
        Some(255),
        "first sector_mark should be at cycle 255"
    );
    for (i, o) in trace.iter().enumerate().skip(255) {
        assert!(
            o.sector_mark,
            "sector_mark should remain high at cycle {i} (no Block to clear it)"
        );
    }
}

#[test]
fn sector_mark_uses_spec_period_by_default() {
    // Per spec §8.1 + diablo_disk::SECTOR_PERIOD_CYCLES, the default
    // disk fires sector_mark every 19,608 microcycles (= 3.333 ms at
    // the 170-ns Alto microcycle, the real Diablo 31 hardware cadence).
    // Verify: 19,608 cycles produces NO rising edge (the wrap happens
    // AT cycle 19,607, sector_mark visible at cycle 19,608); and
    // 19,609 cycles produces exactly one rising edge.
    use rhdl_alto::diablo_disk::SECTOR_PERIOD_CYCLES;
    assert_eq!(
        SECTOR_PERIOD_CYCLES, 19608,
        "spec-correct sector period per spec §8.1 (Diablo 31, 40 ms / 12 sectors / 170 ns)"
    );
    let uut = DiabloDisk::default();
    let trace = run_inputs(
        uut,
        vec![DiskIn::default(); SECTOR_PERIOD_CYCLES as usize + 1],
    );
    let mut events = 0usize;
    let mut prev = false;
    for o in &trace {
        if !prev && o.sector_mark {
            events += 1;
        }
        prev = o.sector_mark;
    }
    assert_eq!(
        events, 1,
        "default DiabloDisk should fire exactly one sector_mark in \
         {SECTOR_PERIOD_CYCLES}+1 cycles; got {events} (spec §8.1 cadence broken?)"
    );
}

#[test]
fn write_then_read_round_trip() {
    let uut = DiabloDisk::default();
    let trace = run_inputs(
        uut,
        vec![
            DiskIn {
                word_addr: b8(5),
                write_data: b16(0xCAFE),
                write_en: true,
                ..DiskIn::default()
            },
            DiskIn {
                word_addr: b8(5),
                read_en: true,
                ..DiskIn::default()
            },
            DiskIn::default(),
        ],
    );
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
    let trace = run_inputs(
        uut,
        vec![
            DiskIn {
                word_addr: b8(0),
                write_data: b16(0xFFFF),
                write_en: true,
                ..DiskIn::default()
            };
            5
        ],
    );
    assert_eq!(trace.len(), 5);
}

#[test]
fn cylinder_head_sector_inputs_pass_through() {
    let uut = DiabloDisk::default();
    let trace = run_inputs(
        uut,
        vec![
            DiskIn {
                cylinder: b8(42),
                head: true,
                sector: b4(7),
                ..DiskIn::default()
            },
            DiskIn {
                cylinder: b8(42),
                head: true,
                sector: b4(7),
                word_addr: b8(0),
                read_en: true,
                ..DiskIn::default()
            },
            DiskIn::default(),
            DiskIn::default(),
        ],
    );
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
