//! Sector_mark / BLOCK / wakeup-clear cadence test.
//!
//! Per the user's review of the wakeup-latched / BLOCK-clear interaction
//! (see CHANGELOG entry "Sector-mark/BLOCK cadence test (§11.1
//! follow-up)"), the existing tests for the disk-sector path only
//! check "fired at least once" — they do NOT catch:
//!
//! - Latch stuck (sector_mark goes high once, never re-rises).
//! - Wrong cadence (sector_mark fires at the wrong period).
//! - BLOCK doesn't clear the latch (KSEC executes F1=Block but
//!   sector_wake stays asserted).
//! - KSEC fires for too long after BLOCK (latch should clear within
//!   1 cycle of BLOCK in current_task=4).
//!
//! This test captures the per-cycle tuple
//!   `(cycle, current_task, block_task, sector_mark, wakeups[4])`
//! over a 5,000-cycle window and asserts the spec §5.5 chain:
//!
//!   sector_tick wraps  →  sector_wake (latch) goes high
//!                      →  wakeups[4] goes high
//!                      →  KSEC (Task 4) wins arbitration
//!                      →  KSEC microcode reaches F1=Block
//!                      →  sector_wake clears (within 1 cycle)
//!
//! See `crates/rhdl-alto/tests/contralto_lockstep.rs` for the
//! ContrAlto cross-validation; this test is the rhdl-alto-only
//! self-consistency anchor that catches latch / cadence regressions
//! independently of ContrAlto availability.

use rhdl::prelude::*;
use rhdl_alto::alto_chip::{AltoChip, ChipIn};
use rhdl_alto::microcode_loader;
use std::path::PathBuf;

const TEST_DISK_PERIOD: u32 = 256;

fn boot_chip() -> Option<AltoChip> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("rom");
    if !dir.join("U55").exists() || !dir.join("C0").exists() {
        return None;
    }
    let microcode = microcode_loader::load_alto_ii_microcode_from_dir(&dir).ok()?;
    let constants = microcode_loader::load_alto_ii_constant_rom_from_dir(&dir).ok()?;
    Some(AltoChip::with_microcode_constants_and_test_disk_period(
        &microcode,
        &constants,
        TEST_DISK_PERIOD,
    ))
}

#[derive(Debug, Clone)]
struct CycleTuple {
    cycle: usize,
    current_task: u8,
    block_task: bool,
    sector_mark: bool,
    wakeup_disk_sector: bool,
}

fn capture(uut: AltoChip, cycles: usize) -> Vec<CycleTuple> {
    let inputs: Vec<ChipIn> = (0..cycles)
        .map(|_| ChipIn {
            wakeups: bits::<16>(0x0001),
        })
        .collect();
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    uut.run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .enumerate()
        .map(|(cycle, t)| CycleTuple {
            cycle,
            current_task: t.current_task.raw() as u8,
            block_task: t.block_task,
            sector_mark: t.disk_sector_mark,
            wakeup_disk_sector: (t.wakeups.raw() & 0x0010) != 0,
        })
        .collect()
}

/// **Cadence test (cadence).**  The configured test disk period is
/// 256 cycles; sector_mark should rise at cycle 255, then again 256
/// cycles after each falling edge.  In a 5,000-cycle window with
/// active KSEC, expect ~5000/256 ≈ 19 ± 2 sector boundaries.
#[test]
fn sector_mark_cadence_matches_test_period() {
    let Some(uut) = boot_chip() else {
        eprintln!("[skip] PROM assets missing");
        return;
    };
    let trace = capture(uut, 5000);

    // Count sector_mark rising edges (false→true).
    let mut rising_edges: Vec<usize> = Vec::new();
    let mut prev = false;
    for t in &trace {
        if !prev && t.sector_mark {
            rising_edges.push(t.cycle);
        }
        prev = t.sector_mark;
    }
    eprintln!(
        "[cadence] {} sector_mark rising edges in 5000 cycles",
        rising_edges.len()
    );
    eprintln!(
        "[cadence] first 10 rising-edge cycles: {:?}",
        rising_edges.iter().take(10).collect::<Vec<_>>()
    );
    let n = rising_edges.len();
    assert!(
        n >= 15 && n <= 22,
        "expected 15..22 sector_mark rising edges in 5000 cycles at \
         {TEST_DISK_PERIOD}-cycle period; got {n} — cadence broken or \
         BLOCK not clearing the latch"
    );

    // Inter-edge spacings should hover around TEST_DISK_PERIOD.  Allow
    // ±32 cycles slack to absorb KSEC's microcode runtime between
    // BLOCK and the next sector boundary (KSEC may take 5-30 cycles
    // to execute its handler before yielding).
    if rising_edges.len() >= 2 {
        let spacings: Vec<i64> = rising_edges
            .windows(2)
            .map(|w| (w[1] as i64) - (w[0] as i64))
            .collect();
        let min_spacing = *spacings.iter().min().unwrap();
        let max_spacing = *spacings.iter().max().unwrap();
        eprintln!("[cadence] inter-edge spacings: min={min_spacing}, max={max_spacing}");
        let target = TEST_DISK_PERIOD as i64;
        assert!(
            (target - 32..=target + 32).contains(&min_spacing)
                && (target - 32..=target + 32).contains(&max_spacing),
            "inter-rising-edge spacings should be near {target}; got \
             min={min_spacing} max={max_spacing}",
        );
    }
}

/// **Latch test (sector_wake → wakeups bit 4).**  Whenever sector_mark
/// is high, wakeups bit 4 must also be high (the disk's sector_mark
/// feeds the chip's effective wakeup vector).  An off-by-one between
/// the disk DFF and the chip's combinational OR would break this.
#[test]
fn sector_mark_drives_wakeup_bit_4() {
    let Some(uut) = boot_chip() else {
        eprintln!("[skip] PROM assets missing");
        return;
    };
    let trace = capture(uut, 5000);
    for t in &trace {
        if t.sector_mark {
            assert!(
                t.wakeup_disk_sector,
                "cycle {}: sector_mark=true but wakeups[4]=false — \
                 disk-to-chip wakeup wiring broken",
                t.cycle
            );
        }
    }
}

/// **BLOCK clears latch test.**  When KSEC executes F1=Block (i.e.,
/// when current_task=4 AND block_task=true), the sector_wake latch
/// should clear within 1 cycle.  Concretely: every cycle where
/// (current_task=4 AND block_task=true) must be followed by a cycle
/// where sector_mark is false (or where current_task changed).
#[test]
fn block_in_ksec_clears_sector_wake_within_one_cycle() {
    let Some(uut) = boot_chip() else {
        eprintln!("[skip] PROM assets missing");
        return;
    };
    let trace = capture(uut, 5000);

    let mut block_events = 0usize;
    let mut block_cleared_latch = 0usize;
    for w in trace.windows(2) {
        let cur = &w[0];
        let next = &w[1];
        if cur.current_task == 4 && cur.block_task {
            block_events += 1;
            if !next.sector_mark {
                block_cleared_latch += 1;
            }
        }
    }
    eprintln!(
        "[block] {block_events} (current_task=4, block_task=true) events; \
              {block_cleared_latch} cleared sector_mark next cycle"
    );
    // Some BLOCK events may overlap with sector_mark already being
    // low.  But the MAJORITY (= every BLOCK that happened while
    // sector_mark was still high) MUST clear the latch.  Simplest
    // soundness check: at least one BLOCK clears the latch (proves
    // the path works), and every BLOCK is followed by !sector_mark.
    assert!(
        block_events > 0,
        "in a 5000-cycle KSEC-active trace, KSEC microcode should \
         have executed F1=Block at least once; got {block_events}"
    );
    assert_eq!(
        block_events, block_cleared_latch,
        "every (current_task=4, block_task=true) cycle should clear \
         sector_mark on the next cycle; got {block_cleared_latch} of \
         {block_events} — BLOCK-clear path is broken or has a delay"
    );
}

/// **Latch-not-stuck test.**  In a 5,000-cycle window with normal
/// KSEC behavior, sector_mark must FALL at least 5 times.  A latch
/// that goes high once and never falls (or falls only once) would
/// indicate the BLOCK-clear path or the latch's reset semantics are
/// broken.  This catches the original "fires once, stays stuck"
/// regression that motivated this test.
#[test]
fn sector_mark_falls_repeatedly_not_stuck() {
    let Some(uut) = boot_chip() else {
        eprintln!("[skip] PROM assets missing");
        return;
    };
    let trace = capture(uut, 5000);

    let mut falling_edges = 0usize;
    let mut prev = false;
    for t in &trace {
        if prev && !t.sector_mark {
            falling_edges += 1;
        }
        prev = t.sector_mark;
    }
    eprintln!("[latch] sector_mark fell {falling_edges} times in 5000 cycles");
    assert!(
        falling_edges >= 5,
        "sector_mark should fall at least 5 times in a 5000-cycle \
         trace at 256-cycle period (~19 cycles between rising edges \
         + KSEC's BLOCK clearing the latch); got {falling_edges} — \
         latch is stuck or BLOCK never executes"
    );
}
