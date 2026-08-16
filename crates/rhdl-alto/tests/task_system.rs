//! Tests for the 16-task wakeup arbiter at the ContrAlto-numbered
//! task slots (0/4/7/8/9/10/11/12/13/14).
//!
//! Demonstrates the BSV-style priority-arbitrated rule pattern:
//! 10 rules sharing the same DFF state; the one with the highest
//! priority and a guarded-true wakeup wins each cycle.

use rhdl::core::sim::ResetOrData;
use rhdl::prelude::*;
use rhdl_alto::task_system::{AltoIn, AltoOut, AltoTaskSystem};

/// Build an `AltoIn` that wakes a chosen set of tasks and supplies
/// a per-task next-MPC = 100 + task_idx so we can tell which task
/// fired by inspecting the resulting MPC change.
fn input(wakeups: u16) -> AltoIn {
    let mut next: [Bits<10>; 16] = [bits::<10>(0); 16];
    for (i, slot) in next.iter_mut().enumerate() {
        *slot = bits::<10>((i as u128) + 100);
    }
    AltoIn {
        wakeups: bits::<16>(wakeups as u128),
        next_mpc_per_task: next,
    }
}

/// Run the task system for N cycles with the given input each cycle.
fn run(uut: AltoTaskSystem, i: AltoIn, cycles: usize) -> Vec<AltoOut> {
    let mut reset_remaining = 2;
    let mut total = 0;
    let mut trace: Vec<AltoOut> = Vec::new();
    uut.run_fn(
        |out: AltoOut| {
            if reset_remaining > 0 {
                reset_remaining -= 1;
                return Some(ResetOrData::Reset);
            }
            if total >= cycles {
                return None;
            }
            total += 1;
            trace.push(out);
            Some(ResetOrData::Data(i))
        },
        100,
    )
    .for_each(drop);
    trace
}

// Wakeup-bit constants for readability.
const WAKE_TASK_0_EMULATOR: u16 = 0x0001;
const WAKE_TASK_4_DISK_SECT: u16 = 0x0010;
const WAKE_TASK_7_ETHERNET: u16 = 0x0080;
const WAKE_TASK_8_MRT: u16 = 0x0100;
const WAKE_TASK_9_DISP_WORD: u16 = 0x0200;
const WAKE_TASK_10_CURSOR: u16 = 0x0400;
const WAKE_TASK_11_DISP_HOR: u16 = 0x0800;
const WAKE_TASK_12_DISP_VER: u16 = 0x1000;
const WAKE_TASK_13_PARITY: u16 = 0x2000;
const WAKE_TASK_14_DISK_WORD: u16 = 0x4000;

// ---- Single-task wakeup ------------------------------------------

#[test]
fn task_0_runs_when_only_task_0_woken() {
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(WAKE_TASK_0_EMULATOR), 5);
    let final_state = trace.last().unwrap();
    assert_eq!(final_state.last_task.raw(), 0);
    assert_eq!(final_state.task_mpc[0].raw(), 100);
    assert!(final_state.any_running);
}

#[test]
fn task_14_runs_when_only_task_14_woken() {
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(WAKE_TASK_14_DISK_WORD), 5);
    let final_state = trace.last().unwrap();
    assert_eq!(final_state.last_task.raw(), 14);
    assert_eq!(final_state.task_mpc[14].raw(), 114);
}

#[test]
fn task_9_display_word_runs_when_woken() {
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(WAKE_TASK_9_DISP_WORD), 5);
    let final_state = trace.last().unwrap();
    assert_eq!(final_state.last_task.raw(), 9);
    assert_eq!(final_state.task_mpc[9].raw(), 109);
}

#[test]
fn task_8_mrt_runs_when_woken() {
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(WAKE_TASK_8_MRT), 5);
    let final_state = trace.last().unwrap();
    assert_eq!(final_state.last_task.raw(), 8);
    assert_eq!(final_state.task_mpc[8].raw(), 108);
}

// ---- Priority arbitration ----------------------------------------

#[test]
fn higher_priority_task_wins_over_lower() {
    // Task 0 (Emulator, lowest) and Task 14 (Disk Word, highest)
    // both woken.  Task 14 fires every cycle; Task 0 starves.
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(WAKE_TASK_14_DISK_WORD | WAKE_TASK_0_EMULATOR), 5);
    let final_state = trace.last().unwrap();
    assert_eq!(
        final_state.last_task.raw(),
        14,
        "Disk Word should beat Emulator"
    );
    assert_eq!(
        final_state.task_mpc[14].raw(),
        114,
        "Disk Word's MPC should advance"
    );
    assert_eq!(
        final_state.task_mpc[0].raw(),
        0,
        "Emulator's MPC should NOT advance"
    );
}

#[test]
fn task_9_display_word_beats_task_0_emulator() {
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(WAKE_TASK_9_DISP_WORD | WAKE_TASK_0_EMULATOR), 5);
    let final_state = trace.last().unwrap();
    assert_eq!(
        final_state.last_task.raw(),
        9,
        "Display Word should beat Emulator"
    );
    assert_eq!(final_state.task_mpc[9].raw(), 109);
    assert_eq!(final_state.task_mpc[0].raw(), 0);
}

#[test]
fn task_14_disk_word_beats_task_4_disk_sector() {
    // Disk Word (Task 14, priority 0) outranks Disk Sector (Task 4,
    // priority 8) — per-word DMA more urgent than per-sector setup.
    let uut = AltoTaskSystem::default();
    let trace = run(
        uut,
        input(WAKE_TASK_14_DISK_WORD | WAKE_TASK_4_DISK_SECT),
        5,
    );
    let final_state = trace.last().unwrap();
    assert_eq!(
        final_state.last_task.raw(),
        14,
        "Disk Word should beat Disk Sector"
    );
    assert_eq!(final_state.task_mpc[14].raw(), 114);
    assert_eq!(final_state.task_mpc[4].raw(), 0);
}

#[test]
fn all_real_tasks_woken_picks_task_14() {
    // Wake every real task slot.  The highest-priority (Task 14) wins.
    let all_real = WAKE_TASK_0_EMULATOR
        | WAKE_TASK_4_DISK_SECT
        | WAKE_TASK_7_ETHERNET
        | WAKE_TASK_8_MRT
        | WAKE_TASK_9_DISP_WORD
        | WAKE_TASK_10_CURSOR
        | WAKE_TASK_11_DISP_HOR
        | WAKE_TASK_12_DISP_VER
        | WAKE_TASK_13_PARITY
        | WAKE_TASK_14_DISK_WORD;
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(all_real), 5);
    let final_state = trace.last().unwrap();
    assert_eq!(
        final_state.last_task.raw(),
        14,
        "Highest-priority task wins"
    );
    assert_eq!(final_state.task_mpc[14].raw(), 114);
    // No other real-task MPC advanced.
    for t in [0, 4, 7, 8, 9, 10, 11, 12, 13] {
        assert_eq!(
            final_state.task_mpc[t].raw(),
            0,
            "Task {t} shouldn't have advanced"
        );
    }
}

// ---- Wakeup gating -----------------------------------------------

#[test]
fn no_wakeups_means_no_task_runs() {
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(0x0000), 5);
    let final_state = trace.last().unwrap();
    assert!(!final_state.any_running, "no wakeup → engine idle");
    for t in 0..16 {
        assert_eq!(final_state.task_mpc[t].raw(), 0);
    }
}

#[test]
fn unused_task_slots_have_no_rules() {
    // Wake task 1 (unused — no rule registered).  The arbiter should
    // see no firable rule and the engine stays idle.
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(0x0002), 5); // bit 1 = task 1 (unused)
    let final_state = trace.last().unwrap();
    assert!(!final_state.any_running, "task 1 has no rule → engine idle");
    // Same for task 2, 3, 5, 6, 15 (unused slots).
    for unused_bit in [0x0004u16, 0x0008, 0x0020, 0x0040, 0x8000] {
        let trace = run(AltoTaskSystem::default(), input(unused_bit), 5);
        assert!(
            !trace.last().unwrap().any_running,
            "wakeup bit {unused_bit:#x} (unused task) should not fire any rule"
        );
    }
}

// ---- Per-task MPC isolation --------------------------------------

#[test]
fn per_task_mpcs_are_independent() {
    let uut = AltoTaskSystem::default();
    let trace_14 = run(uut, input(WAKE_TASK_14_DISK_WORD), 5);
    assert_eq!(trace_14.last().unwrap().task_mpc[14].raw(), 114);
    // All other MPCs should still be 0.
    for t in 0..16 {
        if t == 14 {
            continue;
        }
        assert_eq!(trace_14.last().unwrap().task_mpc[t].raw(), 0);
    }
}

// ---- Phase-3 specialization: per-task body divergence ------------

#[test]
fn disk_sector_count_only_advances_when_task_4_fires() {
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(WAKE_TASK_4_DISK_SECT), 5);
    let final_state = trace.last().unwrap();
    assert_eq!(final_state.last_task.raw(), 4, "Disk Sector ran");
    assert!(
        final_state.disk_sector_count.raw() > 0,
        "sector counter advanced"
    );
    assert_eq!(
        final_state.disk_word_count.raw(),
        0,
        "word counter must NOT advance when only Task 4 fires"
    );
}

#[test]
fn disk_word_count_only_advances_when_task_14_fires() {
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(WAKE_TASK_14_DISK_WORD), 5);
    let final_state = trace.last().unwrap();
    assert_eq!(final_state.last_task.raw(), 14, "Disk Word ran");
    assert!(
        final_state.disk_word_count.raw() > 0,
        "word counter advanced"
    );
    assert_eq!(
        final_state.disk_sector_count.raw(),
        0,
        "sector counter must NOT advance when only Task 14 fires"
    );
}

#[test]
fn other_tasks_do_not_touch_disk_counters() {
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(WAKE_TASK_9_DISP_WORD), 5);
    let final_state = trace.last().unwrap();
    assert_eq!(final_state.last_task.raw(), 9);
    assert_eq!(final_state.disk_sector_count.raw(), 0);
    assert_eq!(final_state.disk_word_count.raw(), 0);
}

#[test]
fn disk_word_priority_wins_when_both_disk_tasks_woken() {
    let uut = AltoTaskSystem::default();
    let trace = run(
        uut,
        input(WAKE_TASK_14_DISK_WORD | WAKE_TASK_4_DISK_SECT),
        5,
    );
    let final_state = trace.last().unwrap();
    assert_eq!(final_state.last_task.raw(), 14, "Disk Word wins");
    assert!(final_state.disk_word_count.raw() > 0);
    assert_eq!(
        final_state.disk_sector_count.raw(),
        0,
        "sector counter starves under Disk Word priority"
    );
}

// ---- iverilog round-trip — proves rhdl-rule lowering is real
// hardware, not just a Rust-only simulation.

#[test]
fn task_system_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut = AltoTaskSystem::default();
    // Cycle through the real task wakeup bits.
    let real_wake: [u16; 10] = [
        WAKE_TASK_0_EMULATOR,
        WAKE_TASK_4_DISK_SECT,
        WAKE_TASK_7_ETHERNET,
        WAKE_TASK_8_MRT,
        WAKE_TASK_9_DISP_WORD,
        WAKE_TASK_10_CURSOR,
        WAKE_TASK_11_DISP_HOR,
        WAKE_TASK_12_DISP_VER,
        WAKE_TASK_13_PARITY,
        WAKE_TASK_14_DISK_WORD,
    ];
    let inputs: Vec<AltoIn> = (0..6)
        .map(|cyc| input(real_wake[cyc % real_wake.len()]))
        .collect();
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    let tm = test_bench.ntl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
