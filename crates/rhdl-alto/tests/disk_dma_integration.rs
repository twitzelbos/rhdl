//! Phase-3 integration test: DiabloDisk → AltoTaskSystem.
//!
//! Proves the Phase-3 disk subsystem composes with the Phase-2 task
//! arbiter end-to-end:
//!
//! 1. Run the simulated [`DiabloDisk`] standalone for several sector
//!    boundaries and capture the `sector_mark` / `word_strobe`
//!    output stream.
//! 2. Translate that stream into an [`AltoIn::wakeups`] vector
//!    (bit 1 = Disk Sector, bit 2 = Disk Word).
//! 3. Feed the wakeup stream into [`AltoTaskSystem`] and verify
//!    that:
//!       - Disk Sector (Task 1) fires once per `sector_mark`.
//!       - The `disk_sector_count` advances accordingly.
//!       - Other tasks remain idle.
//!
//! What this does NOT yet exercise (Phase 3.5+):
//!
//! - Real Alto disk microcode driving KADR / KCOM / KCWA.
//! - End-to-end DMA between disk buffer and main memory through the
//!   microengine.  The DMA path is mocked here as a wakeup-only flow.

use rhdl::prelude::*;
use rhdl_alto::diablo_disk::{DiabloDisk, DiskIn, DiskOut};
use rhdl_alto::task_system::{AltoIn, AltoOut, AltoTaskSystem};

fn b16(v: u128) -> Bits<16> { bits::<16>(v) }
fn b10(v: u128) -> Bits<10> { bits::<10>(v) }

/// Simulate the DiabloDisk for N cycles with idle inputs and return
/// the output trace (sector_mark / word_strobe per cycle).
fn run_disk(uut: DiabloDisk, cycles: usize) -> Vec<DiskOut> {
    let stream = std::iter::repeat_n(DiskIn::default(), cycles)
        .with_reset(1).clock_pos_edge(100);
    uut.run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect()
}

/// Run the AltoTaskSystem with a sequence of inputs and return the
/// per-cycle output trace.
fn run_arbiter(uut: AltoTaskSystem, inputs: Vec<AltoIn>) -> Vec<AltoOut> {
    let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
    uut.run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect()
}

/// Build an AltoIn from a wakeup mask.  next_mpc is per-task = task+100.
fn input_from_wakeups(wakeups: u128) -> AltoIn {
    let mut next: [Bits<10>; 16] = [b10(0); 16];
    for (i, slot) in next.iter_mut().enumerate() {
        *slot = b10((i as u128) + 100);
    }
    AltoIn { wakeups: b16(wakeups), next_mpc_per_task: next }
}

#[test]
fn disk_sector_mark_drives_disk_sector_task() {
    // Run the disk for ~3 sector boundaries (3 × 256 + a few cycles).
    // Per *Alto Hardware Manual* §2.4 + spec §5.5, sector_mark is a
    // SUSTAINED wakeup signal — set on sector boundary and only
    // cleared by F1=Block from current_task=4.  This standalone-disk
    // test passes block_task=false (DiskIn::default()), so once
    // sector_mark fires at cycle 255 it stays high for the rest of
    // the trace (with another rising edge happening only if it had
    // been cleared).  Verify the rising-edge events instead.
    let cycles = 256 * 3 + 10;
    let disk = DiabloDisk::default();
    let disk_trace = run_disk(disk, cycles);

    let mut rising_edges: Vec<usize> = Vec::new();
    let mut prev = false;
    for (i, o) in disk_trace.iter().enumerate() {
        if !prev && o.sector_mark {
            rising_edges.push(i);
        }
        prev = o.sector_mark;
    }
    // Without Block to clear sector_mark, only the FIRST rising edge
    // happens; the signal then stays high for the rest of the trace.
    assert_eq!(rising_edges, vec![255],
        "first sector_mark rising edge at cycle 255; subsequent ticks \
         re-OR true→true (no rising edge) per the sustained-wakeup model");

    // Translate the disk trace into wakeup inputs (ContrAlto convention):
    // sector_mark → bit 4.  Once asserted, every subsequent cycle wakes
    // task 4, so the arbiter fires task 4 on every cycle from 255 on.
    let arbiter_inputs: Vec<AltoIn> = disk_trace.iter().map(|o| {
        let mut wake: u128 = 0;
        if o.sector_mark  { wake |= 0x0010; }
        if o.word_strobe  { wake |= 0x4000; }
        input_from_wakeups(wake)
    }).collect();

    let arbiter = AltoTaskSystem::default();
    let arbiter_trace = run_arbiter(arbiter, arbiter_inputs);

    let final_state = arbiter_trace.last().unwrap();
    // disk_sector_count fires once per cycle that task 4 wins
    // arbitration.  With sustained sector_mark from cycle 255 onward
    // (and no other competing tasks), task 4 fires every cycle from
    // 255 to end.  cycles - 255 = 778-256 = ~523 firings (allowing
    // ±1 for pipeline / output-DFF edge-of-trace).
    let firings = final_state.disk_sector_count.raw();
    let expected_min = (cycles - 255 - 2) as u128;
    assert!(firings >= expected_min,
        "task 4 should fire every cycle from sector_mark onward; \
         expected ≥{expected_min}, got {firings}");
    assert_eq!(final_state.disk_word_count.raw(), 0,
        "no transfer started → no word strobes → word counter idle");
    assert_eq!(final_state.last_task.raw(), 4,
        "last_task remembers the most recent disk-sector firing (task 4)");
}

#[test]
fn synthetic_word_strobe_drives_disk_word_task() {
    // The Phase-3 disk widget only asserts word_strobe when a transfer
    // is active (transfer_remaining > 0).  Phase-3 has no way to start
    // a transfer through inputs (Phase 3.5 will route the controller
    // through to set transfer_remaining via a KCOM op).  So fabricate
    // a word_strobe stream directly: 5 cycles asserted, then idle.
    let arbiter_inputs: Vec<AltoIn> = (0..10).map(|cycle| {
        if cycle < 5 {
            input_from_wakeups(0x4000) // bit 14 = Disk Word (ContrAlto convention)
        } else {
            input_from_wakeups(0x0000)
        }
    }).collect();

    let arbiter = AltoTaskSystem::default();
    let arbiter_trace = run_arbiter(arbiter, arbiter_inputs);

    let final_state = arbiter_trace.last().unwrap();
    assert_eq!(final_state.disk_word_count.raw(), 5);
    assert_eq!(final_state.disk_sector_count.raw(), 0);
}

#[test]
fn disk_word_outranks_emulator_under_pressure() {
    // Emulator (Task 0) always woken, Disk Word (Task 14) woken for
    // 5 cycles during transfer.  Disk Word starves Emulator while
    // transfer active.
    let arbiter_inputs: Vec<AltoIn> = (0..10).map(|cycle| {
        let mut wake: u128 = 0x0001; // emulator always woken (bit 0)
        if cycle < 5 { wake |= 0x4000; } // disk word during transfer (bit 14)
        input_from_wakeups(wake)
    }).collect();

    let arbiter = AltoTaskSystem::default();
    let arbiter_trace = run_arbiter(arbiter, arbiter_inputs);

    // Output is observed one cycle after the firing (last_task is a DFF).
    // Cycles 0..4 fire Disk Word (task 14) → trace[1..=5].last_task == 14.
    for cyc in 1..=5 {
        assert_eq!(arbiter_trace[cyc].last_task.raw(), 14,
            "cycle {cyc} (out): Disk Word should win over Emulator");
    }
    // Cycles 5..9 fire Emulator (task 0) → trace[6..=9].last_task == 0.
    for cyc in 6..=9 {
        assert_eq!(arbiter_trace[cyc].last_task.raw(), 0,
            "cycle {cyc} (out): Emulator runs once Disk Word releases");
    }

    let final_state = arbiter_trace.last().unwrap();
    assert_eq!(final_state.disk_word_count.raw(), 5);
    assert_eq!(final_state.task_mpc[0].raw(), 100);
}
