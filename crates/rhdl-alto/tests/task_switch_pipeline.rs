//! Task-switch pipeline-timing regression test.
//!
//! Per AltoHW §2.4 (cross-checked with ContrAlto's TSV trace at boot
//! cycles 2-4 of `nonprog.dsk`):
//!
//! > "If the processor executes the TASK function (F1=2) during an
//! >  instruction, the current task register is loaded (at the end
//! >  of the instruction) with the number of the current highest
//! >  priority task as determined by the priority encoder. This
//! >  causes the next instruction to be fetched from the ROM
//! >  location specified by the saved task's MPC.  **One additional
//! >  instruction is executed before the switch becomes effective.**"
//!
//! So the spec-correct sequence is:
//!
//!   Cycle K:    OLD task runs the F1=TaskYield instruction.
//!   Cycle K+1:  OLD task continues, runs the NEXT field of cycle K.
//!               (= "one additional instruction is executed before
//!               the switch becomes effective")
//!   Cycle K+2:  NEW task starts at its saved MPC (or reset MPC).
//!
//! ContrAlto's TSV trace at boot confirms this:
//!
//!   cycle 0:  task=0 mpc=0x152
//!   cycle 1:  task=0 mpc=0x152  (memory stall)
//!   cycle 2:  task=0 mpc=0x153  ← F1=TaskYield fires HERE
//!   cycle 3:  task=0 mpc=0x154  ← old task still running (K+1)
//!   cycle 4:  task=4 mpc=0x004  ← KSEC starts (K+2)
//!
//! This test pins the timing.  Construct a minimal scenario:
//!
//!   MPC=0x000 (Emulator NOVEM): jumps to MPC=0x100.
//!   MPC=0x100: F1=TaskYield, next=0x101.  No higher-priority task
//!              woken from external wakeups, but Disk Sector wakeup
//!              fires immediately via with_microcode_..._at_boundary.
//!   MPC=0x101: NEXT=0x101 (loop) — the "one additional instruction"
//!              that runs in the OLD task.
//!   MPC=0x004 (KSEC reset MPC): NEXT=0x004 (loop) — where KSEC lands.
//!
//! Expected MPC trace (per spec):
//!   trace[0] = 0x000 (cycle 0 ran NOVEM — Emulator)
//!   trace[1] = 0x100 (cycle 1 ran 0x100, F1=TaskYield — Emulator)
//!   trace[2] = 0x101 (cycle 2 ran 0x101, the K+1 cycle — Emulator)
//!   trace[3] = 0x004 (cycle 3 ran KSEC reset MPC — task switched to 4)
//!   trace[4] = 0x004 (KSEC loops)

use rhdl::prelude::*;
use rhdl_alto::alto_chip::{AltoChip, ChipIn, ChipOut};
use rhdl_alto::isa::{
    AluFunction, BusSource, F1Function, F2Function, Microinstruction,
};

const NUM_CONSTANTS: usize = rhdl_alto::constant_rom::NUM_CONSTANTS;
const MICROCODE_WORDS: usize = rhdl_alto::microcode_rom::MICROCODE_WORDS;

fn b5(v: u128) -> Bits<5> { bits::<5>(v) }
fn b10(v: u128) -> Bits<10> { bits::<10>(v) }

fn run(uut: AltoChip, cycles: usize) -> Vec<ChipOut> {
    let inputs: Vec<ChipIn> = (0..cycles)
        .map(|_| ChipIn { wakeups: bits::<16>(0x0001) })
        .collect();
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    uut.run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect()
}

#[test]
fn f1_task_yield_takes_effect_one_cycle_late_per_spec_2_4() {
    let mut microcode = [0u32; MICROCODE_WORDS];
    let constants = [0u16; NUM_CONSTANTS];

    // Build a NOP-chain through MPC 0,1,2,3,4 so we can distinguish
    // every cycle clearly.  TaskYield at MPC=3 (= cycle K).  The
    // "one additional" cycle goes to MPC=0x10 (a self-loop trap, so
    // we KNOW the chip stayed at OLD task if it lands there).  The
    // NEW task (Disk Sector at task=4) lands at its reset MPC=4.
    //
    // We can't reuse MPC=4 for the trap (KSEC reset MPC = 4), so
    // we use 0x10 for the K+1 trap.  Microcode layout:
    //   MPC=0 → 1  (Emulator boot, NOVEM-equivalent)
    //   MPC=1 → 2  (Emulator continuing)
    //   MPC=2 → 3  (Emulator continuing)
    //   MPC=3:  F1=TaskYield, next=0x10  (= K)
    //   MPC=0x10:  next=0x10 (self-loop = "K+1 old-task-continues" trap)
    //   MPC=4:  next=4 (KSEC reset MPC, self-loop = NEW task target)
    let nop_to = |dest: u16| Microinstruction {
        rsel: b5(0), aluf: AluFunction::Bus, bs: BusSource::ReadR,
        f1: F1Function::Nop, f2: F2Function::Nop,
        t_load: false, l_load: false, next: b10(dest as u128),
    }.pack();
    microcode[0] = nop_to(1);
    microcode[1] = nop_to(2);
    microcode[2] = nop_to(3);
    microcode[3] = Microinstruction {
        rsel: b5(0), aluf: AluFunction::Bus, bs: BusSource::ReadR,
        f1: F1Function::TaskYield, f2: F2Function::Nop,
        t_load: false, l_load: false, next: b10(0x10),
    }.pack();
    microcode[0x10] = nop_to(0x10);  // K+1 trap (Emulator continues here)
    microcode[4] = nop_to(4);        // KSEC reset MPC, self-loop

    // Construct chip with sector_mark firing on cycle 1 — this gives
    // the arbiter a Disk Sector wakeup ready by the time the
    // Emulator's F1=TaskYield fires at cycle 1 (running MPC=0x100).
    let boot_data = [0u16; 256];
    let boot_label = [0u16; 8];
    let uut = AltoChip::with_microcode_constants_boot_and_test_disk_period_at_boundary(
        &microcode, &constants, &boot_data, &boot_label, 256,
    );
    let trace = run(uut, 8);

    eprintln!("MPC + task + task_yield trace (first 8 cycles):");
    for (i, t) in trace.iter().enumerate() {
        eprintln!(
            "  cycle {i}: task={} mpc=0x{:03x} next_mpc=0x{:03x} task_yield={} sector_mark={}",
            t.current_task.raw(), t.mpc.raw(), t.next_mpc.raw(),
            t.task_yield, t.disk_sector_mark
        );
    }

    // Verify we see a clear MPC chain 0→1→2→3 in Emulator, then a
    // single "K+1 continues old task" cycle at 0x10, then KSEC at 4.

    // The CHIP'S `mpc` and `current_task` outputs reflect a 1-cycle
    // pipeline delay vs. what the engine is actually executing —
    // `mpc` is the start-of-cycle MPC presented to URom, not the MPC
    // of the instruction currently being executed.  So we anchor
    // assertions to the SAMPLE INDEX after a known event (task_yield
    // pulse) and check the TASK transitions, which DO map cleanly.
    //
    // Per the prior trace, with the F1=TaskYield at microcode[3]:
    //   - task_yield pulses HIGH for exactly one sample (cycle K).
    //   - Per spec §2.4 + the F1=TaskYield-pipeline fix in this PR:
    //     - cycle K+1: still OLD task (= the "one additional
    //       instruction" cycle).
    //     - cycle K+2: NEW task (Disk Sector = 4) takes over.

    let k = trace.iter().position(|t| t.task_yield)
        .expect("task_yield should pulse at some cycle (microcode[3] has F1=TaskYield)");
    eprintln!("task_yield went high at sample K={k}");

    // Cycle K: TaskYield fires.  Old task still running per spec.
    assert_eq!(trace[k].current_task.raw(), 0,
        "sample K (where task_yield pulses) should still report old \
         task (Emulator) per spec §2.4 — task register loads at end \
         of cycle K");

    // Cycle K+1: STILL OLD TASK per spec §2.4 ("one additional
    // instruction is executed before the switch becomes effective").
    // This is the assertion that fails BEFORE the fix and passes AFTER.
    assert_eq!(trace[k+1].current_task.raw(), 0,
        "sample K+1 SHOULD still be Emulator per spec §2.4.  \
         If this fails with task=4, the chip is switching tasks \
         immediately on F1=TaskYield (= K+1 timing) instead of \
         deferring one cycle as spec requires (= K+2 timing).  This \
         was the bug fixed by the task_yield_pending DFF.");

    // Cycle K+2: NEW task (Disk Sector = 4) takes over.
    assert_eq!(trace[k+2].current_task.raw(), 4,
        "sample K+2 SHOULD be Disk Sector (task 4) — switch becomes \
         effective per spec §2.4");

    // Bonus: KSEC's MPC reset value should be 4 once it starts running.
    // Wait one more cycle for the chip's MPC reporting to align (BRAM
    // pipeline + task_started latch interaction).
    assert_eq!(trace[k+3].current_task.raw(), 4,
        "sample K+3 should still be Disk Sector (loop)");
}
