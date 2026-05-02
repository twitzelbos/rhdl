//! Microengine integration tests: hand-written Alto microcode
//! programs run through the universal-task microengine.
//!
//! Phase 3.5: MPC is owned externally (by AltoChip/task_system in
//! production; by the test harness here).  Each test:
//!   1. Constructs a small `[Microinstruction; N]` program.
//!   2. Packs it to `[u32; N]`.
//!   3. Runs the microengine, with the harness owning the MPC,
//!      feeding `(mpc, instr=program[mpc])` each cycle, and
//!      committing `out.next_mpc` as the next-cycle MPC.
//!   4. Asserts on observable state (T, L, R-registers, BUS, MPC).

use rhdl::core::sim::ResetOrData;
use rhdl::prelude::*;
use rhdl_alto::isa::*;
use rhdl_alto::microengine::{In, Microengine, Out};

/// Build a Microinstruction with mostly-default fields.  Tests
/// only override the bits they care about.
fn ui(rsel: u8, aluf: AluFunction, bs: BusSource, t_load: bool, l_load: bool, next: u16) -> Microinstruction {
    Microinstruction {
        rsel: bits::<5>(rsel as u128),
        aluf,
        bs,
        f1: F1Function::Nop,
        f2: F2Function::Nop,
        t_load,
        l_load,
        next: bits::<10>(next as u128),
    }
}

/// Run a microcode program for `cycles` clocks.  Returns the
/// per-cycle Out trace.  The harness drives MPC from `out.next_mpc`
/// each cycle (boot starts at MPC=0 since reset sets next_mpc=0).
fn run(program: Vec<u32>, cycles: usize) -> Vec<Out> {
    let uut = Microengine::default();
    let mut reset_remaining = 2;
    let mut total = 0;
    let mut trace: Vec<Out> = Vec::new();
    uut.run_fn(
        |out: Out| {
            if reset_remaining > 0 {
                reset_remaining -= 1;
                return Some(ResetOrData::Reset);
            }
            if total >= cycles { return None; }
            total += 1;
            trace.push(out);
            // Drive next cycle's MPC from this cycle's computed
            // next_mpc.  Reset path leaves next_mpc=0, so the first
            // non-reset cycle gets MPC=0 (boot from address 0).
            let mpc_to_drive = out.next_mpc.raw();
            let instr = if (mpc_to_drive as usize) < program.len() {
                program[mpc_to_drive as usize]
            } else { 0 };
            Some(ResetOrData::Data(In {
                mpc: bits::<10>(mpc_to_drive),
                instr: bits::<32>(instr as u128),
                constant_value: bits::<16>(0),
                mem_read_data: bits::<16>(0),
                current_task: bits::<4>(0),
            }))
        },
        100,
    ).for_each(drop);
    trace
}

// ---- Trivial single-cycle programs ------------------------------

#[test]
fn t_load_from_zero_bus() {
    // Program at addr 0:
    //   T_LOAD = 1, BS = ReadR (R[0] = 0 at reset → bus = 0), NEXT = 0 (loop here)
    //   ALUF = Bus → ALU result = 0
    let prog: Vec<u32> = vec![
        ui(0, AluFunction::Bus, BusSource::ReadR, true, false, 0).pack(),
    ];
    let trace = run(prog, 4);
    // After the T_LOAD commits at cycle 0's edge, cycle 1 reads T = 0.
    // T should remain 0 (we're loading 0 from R[0] = 0).
    for o in &trace {
        assert_eq!(o.t.raw(), 0);
    }
}

#[test]
fn l_load_from_alu_plus_one() {
    // Program at addr 0:
    //   T_LOAD = 0, L_LOAD = 1, BS = ReadR (bus = R[0] = 0),
    //   ALUF = BusPlusOne (result = 1), NEXT = 0 (loop)
    let prog: Vec<u32> = vec![
        ui(0, AluFunction::BusPlusOne, BusSource::ReadR, false, true, 0).pack(),
    ];
    let trace = run(prog, 6);
    // Cycle 0 (post-reset): L = 0 (reset), ALU computes 1; commit at edge.
    // Cycle 1: L = 1.
    // Cycle 2+: L = 1 (idempotent — L gets 1 each cycle).
    assert_eq!(trace[0].l.raw(), 0);
    assert_eq!(trace[1].l.raw(), 1);
    assert_eq!(trace[5].l.raw(), 1);
}

#[test]
fn microengine_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut = Microengine::default();
    // Single-microinstruction program: NOP-ish loop at addr 0.
    let prog: Vec<u32> = vec![
        ui(0, AluFunction::Bus, BusSource::ReadR, false, false, 0).pack(),
    ];
    let inputs: Vec<In> = (0..6).map(|_| In {
        mpc: bits::<10>(0),
        instr: bits::<32>(prog[0] as u128),
        constant_value: bits::<16>(0),
        mem_read_data: bits::<16>(0),
        current_task: bits::<4>(0),
    }).collect();
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}

// ---- A small two-instruction microprogram ------------------------

#[test]
fn two_step_program_t_then_alu() {
    // addr 0: T ← BUS (which is R[0] = 0); NEXT = 1
    // addr 1: ALUF = BusPlusOne, L_LOAD = 1, BS = ReadR (bus = 0),
    //         NEXT = 1 (loop here).  L should latch 1.
    let prog: Vec<u32> = vec![
        ui(0, AluFunction::Bus, BusSource::ReadR, true, false, 1).pack(),
        ui(0, AluFunction::BusPlusOne, BusSource::ReadR, false, true, 1).pack(),
    ];
    let trace = run(prog, 8);
    // Phase-3.5 trace indexing: out is observed AFTER the cycle's
    // edge, so trace[N].mpc = mpc input the harness drove based on
    // out.next_mpc from trace[N-1].  Because reset's o.next_mpc=0,
    // the first processed cycle had input mpc=0; trace[1].mpc=0.
    // The cycle that processed mpc=1 (L_LOAD) gives trace[2].
    assert_eq!(trace[0].mpc.raw(), 0, "post-reset");
    assert_eq!(trace[1].mpc.raw(), 0, "first cycle processed addr 0 (T_LOAD)");
    assert_eq!(trace[2].mpc.raw(), 1, "second cycle processed addr 1 (L_LOAD)");
    assert_eq!(trace[2].l.raw(), 1, "L latched from cycle that processed L_LOAD");
    assert_eq!(trace[7].l.raw(), 1, "L stays 1 in the loop");
}

// ---- F1 shifts ----------------------------------------------------

#[test]
fn left_shift_l_each_cycle() {
    // Setup: load L = 1 at addr 0; then loop at addr 1 doing
    // F1 = LeftShift1, NEXT = 1.  L should double each cycle.
    let prog: Vec<u32> = vec![
        // addr 0: L_LOAD=1, ALUF = BusPlusOne, BS = ReadR (bus = 0) → L = 1
        ui(0, AluFunction::BusPlusOne, BusSource::ReadR, false, true, 1).pack(),
        // addr 1: F1 = LeftShift1, NEXT = 1, no L_LOAD (so the
        //         "candidate L" is the current L; F1 shifts it).
        {
            let mut mi = ui(0, AluFunction::Bus, BusSource::None, false, false, 1);
            mi.f1 = F1Function::LeftShift1;
            mi.pack()
        },
    ];
    let trace = run(prog, 10);
    // After cycle 1, L = 1.  Each subsequent cycle doubles L.
    assert_eq!(trace[1].l.raw(), 1, "L should be 1 after first instr commits");
    // Cycle 2: L was 1, shift left → L = 2.
    // Cycle 3: L = 4.  Cycle 4: L = 8.  ...
    assert_eq!(trace[2].l.raw(), 2);
    assert_eq!(trace[3].l.raw(), 4);
    assert_eq!(trace[4].l.raw(), 8);
    assert_eq!(trace[5].l.raw(), 16);
}

// ---- Branch via F2 = BusEqZero -----------------------------------

#[test]
fn branch_on_bus_eq_zero() {
    // Program: at addr 0, ALUF = BusPlusOne, L_LOAD=1, BS = ReadR
    //          (bus = R[0] = 0), F2 = BusEqZero, NEXT = 0b0000000010 = 2.
    //          Since bus == 0, F2 sets bit 0 of NEXT → next addr = 3.
    // addr 3: terminating loop.  L should be 1 throughout.
    let prog: Vec<u32> = vec![
        {
            let mut mi = ui(0, AluFunction::BusPlusOne, BusSource::ReadR, false, true, 2);
            mi.f2 = F2Function::BusEqZero;
            mi.pack()
        },
        // addr 1, 2: NOP fillers.
        ui(0, AluFunction::Bus, BusSource::None, false, false, 1).pack(),
        ui(0, AluFunction::Bus, BusSource::None, false, false, 2).pack(),
        // addr 3: loop here.
        ui(0, AluFunction::Bus, BusSource::None, false, false, 3).pack(),
    ];
    let trace = run(prog, 8);
    // trace[0]: post-reset (mpc=0).
    // trace[1]: first cycle processed mpc=0 (BusEqZero); o.next_mpc=3
    //          (skip to addr 3).  L_LOAD took effect → l=1 next cycle.
    // trace[2]: cycle processed mpc=3 (loop).
    assert_eq!(trace[0].mpc.raw(), 0, "post-reset");
    assert_eq!(trace[1].mpc.raw(), 0, "first cycle processed addr 0 (BusEqZero)");
    assert_eq!(trace[1].next_mpc.raw(), 3, "BusEqZero should compute next_mpc=3");
    assert_eq!(trace[2].mpc.raw(), 3, "second cycle processed addr 3 (branch target)");
    assert_eq!(trace[3].mpc.raw(), 3, "should loop at addr 3");
    assert_eq!(trace[1].l.raw(), 1, "L latched from cycle 0's ALU bus+1");
}
