//! Tests for the 16-task wakeup arbiter expressed as a [`rhdl_rule`]
//! kernel.
//!
//! Demonstrates the BSV-style priority-arbitrated rule pattern:
//! 16 rules sharing the same DFF state; the one with the highest
//! priority and a guarded-true wakeup wins each cycle.

use rhdl::core::sim::ResetOrData;
use rhdl::prelude::*;
use rhdl_alto::task_system::{AltoIn, AltoOut, AltoTaskSystem};

/// Build an `AltoIn` that wakes a chosen set of tasks and supplies
/// a per-task next-MPC = (task_idx + 1) so we can tell which task
/// fired by inspecting the resulting MPC change.
fn input(wakeups: u16) -> AltoIn {
    let mut next: [Bits<10>; 16] = [bits::<10>(0); 16];
    for i in 0..16 {
        next[i] = bits::<10>((i as u128) + 100);
    }
    AltoIn {
        wakeups: bits::<16>(wakeups as u128),
        next_mpc_per_task: next,
    }
}

/// Run the task system for N cycles with the given input each cycle.
/// Returns the per-cycle Out trace.
fn run(uut: AltoTaskSystem, i: AltoIn, cycles: usize) -> Vec<AltoOut> {
    let mut reset_remaining = 2;
    let mut total = 0;
    let mut trace: Vec<AltoOut> = Vec::new();
    uut.run_fn(
        |out: AltoOut| {
            if reset_remaining > 0 { reset_remaining -= 1; return Some(ResetOrData::Reset); }
            if total >= cycles { return None; }
            total += 1;
            trace.push(out);
            Some(ResetOrData::Data(i))
        },
        100,
    ).for_each(drop);
    trace
}

// ---- Single-task wakeup -------------------------------------------

#[test]
fn task_0_runs_when_only_task_0_woken() {
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(0x0001), 5);
    // After cycle 0's commit, last_task should be 0 (Emulator).
    // task_mpc[0] should be 100 (the next_mpc we supplied).
    let final_state = trace.last().unwrap();
    assert_eq!(final_state.last_task.raw(), 0);
    assert_eq!(final_state.task_mpc[0].raw(), 100);
    assert!(final_state.any_running);
}

#[test]
fn task_15_runs_when_only_task_15_woken() {
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(0x8000), 5);
    let final_state = trace.last().unwrap();
    assert_eq!(final_state.last_task.raw(), 15);
    assert_eq!(final_state.task_mpc[15].raw(), 115);
}

#[test]
fn task_5_display_word_runs_when_woken() {
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(0x0020), 5);
    let final_state = trace.last().unwrap();
    assert_eq!(final_state.last_task.raw(), 5);
    assert_eq!(final_state.task_mpc[5].raw(), 105);
}

// ---- Priority arbitration -----------------------------------------

#[test]
fn higher_priority_task_wins_over_lower() {
    // Task 0 (Emulator) and Task 15 both woken.  Task 15 has higher
    // priority → Task 15 fires every cycle; Task 0 starves.
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(0x8001), 5);
    let final_state = trace.last().unwrap();
    assert_eq!(final_state.last_task.raw(), 15, "Task 15 should beat Task 0");
    assert_eq!(final_state.task_mpc[15].raw(), 115, "Task 15's MPC should advance");
    assert_eq!(final_state.task_mpc[0].raw(), 0, "Task 0's MPC should NOT advance");
}

#[test]
fn task_5_beats_task_0_on_simultaneous_wakeup() {
    // Display Word (Task 5) outranks Emulator (Task 0).
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(0x0021), 5);
    let final_state = trace.last().unwrap();
    assert_eq!(final_state.last_task.raw(), 5, "Display Word should beat Emulator");
    assert_eq!(final_state.task_mpc[5].raw(), 105);
    assert_eq!(final_state.task_mpc[0].raw(), 0);
}

#[test]
fn task_2_disk_word_beats_task_1_disk_sector() {
    // Disk Word (Task 2) is higher-priority than Disk Sector (Task 1)
    // — per-word DMA is more urgent than per-sector setup.
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(0x0006), 5);
    let final_state = trace.last().unwrap();
    assert_eq!(final_state.last_task.raw(), 2, "Disk Word should beat Disk Sector");
    assert_eq!(final_state.task_mpc[2].raw(), 102);
    assert_eq!(final_state.task_mpc[1].raw(), 0);
}

#[test]
fn all_16_woken_picks_task_15() {
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(0xFFFF), 5);
    let final_state = trace.last().unwrap();
    assert_eq!(final_state.last_task.raw(), 15, "Highest-priority task wins");
    // Only Task 15's MPC advanced; the other 15 task MPCs stayed at 0.
    assert_eq!(final_state.task_mpc[15].raw(), 115);
    for i in 0..15 {
        assert_eq!(final_state.task_mpc[i].raw(), 0,
            "Task {i} shouldn't have advanced");
    }
}

// ---- Wakeup gating -----------------------------------------------

#[test]
fn no_wakeups_means_no_task_runs() {
    let uut = AltoTaskSystem::default();
    let trace = run(uut, input(0x0000), 5);
    let final_state = trace.last().unwrap();
    assert!(!final_state.any_running, "no wakeup → engine idle");
    for i in 0..16 {
        assert_eq!(final_state.task_mpc[i].raw(), 0);
    }
}

// ---- Per-task MPC isolation ---------------------------------------

#[test]
fn per_task_mpcs_are_independent() {
    // Run Task 15 for several cycles to advance its MPC.
    let uut = AltoTaskSystem::default();
    let trace_15 = run(uut, input(0x8000), 5);
    assert_eq!(trace_15.last().unwrap().task_mpc[15].raw(), 115);
    // All other MPCs should still be 0.
    for i in 0..15 {
        assert_eq!(trace_15.last().unwrap().task_mpc[i].raw(), 0);
    }
}

// ---- iverilog round-trip — proves the rhdl-rule lowering is real
// hardware, not just a Rust-only simulation.

#[test]
fn task_system_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut = AltoTaskSystem::default();
    let inputs: Vec<AltoIn> = (0..6).map(|cycle| input(1 << ((cycle as u16) & 0xF))).collect();
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    let tm = test_bench.ntl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
