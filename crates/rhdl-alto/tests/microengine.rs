//! Microengine integration tests: hand-written Alto microcode
//! programs run through the 2-stage MIF/MIE pipeline.
//!
//! Each test:
//!   1. Constructs a small `[Microinstruction; N]` program.
//!   2. Packs it to `[u32; N]`.
//!   3. Runs the microengine, with the harness serving microcode
//!      RAM combinationally based on `out.mpc`.
//!   4. Asserts on observable state (T, L, R-registers, BUS).

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
/// per-cycle Out trace.
fn run(program: Vec<u32>, cycles: usize) -> Vec<Out> {
    let uut = Microengine::default();
    let mut reset_remaining = 2;
    let mut total = 0;
    let mut trace: Vec<Out> = Vec::new();
    uut.run_fn(
        |out: Out| {
            if reset_remaining > 0 { reset_remaining -= 1; return Some(ResetOrData::Reset); }
            if total >= cycles { return None; }
            total += 1;
            trace.push(out);
            let mpc = out.mpc.raw() as usize;
            let instr = if mpc < program.len() { program[mpc] } else { 0 };
            Some(ResetOrData::Data(In { instr: bits::<32>(instr as u128) }))
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
    // Cycle 0: L = 0 (reset), ALU computes 1; commit at edge.
    // Cycle 1: L = 1.
    // Cycle 2+: L = 1 (idempotent — L gets 1 each cycle).
    assert_eq!(trace[0].l.raw(), 0);
    assert_eq!(trace[1].l.raw(), 1);
    assert_eq!(trace[5].l.raw(), 1);
}

#[test]
fn microengine_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut = Microengine::default();
    // Single-microinstruction program: NOP-ish loop.
    let prog: Vec<u32> = vec![
        ui(0, AluFunction::Bus, BusSource::ReadR, false, false, 0).pack(),
    ];
    let inputs: Vec<In> = (0..6).map(|cycle| {
        // Without read-back of mpc, just feed program[0] every cycle.
        In { instr: bits::<32>(prog[0] as u128) }
    }).collect();
    let _ = cycle_unused();
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}

#[allow(unused)]
fn cycle_unused() -> usize { 0 }

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
    // Cycle 0: at addr 0 (T_LOAD); commit T=0.
    // Cycle 1: at addr 1 (L_LOAD with BUS+1); commit L=1.
    // Cycle 2+: at addr 1 (loop); L stays 1.
    assert_eq!(trace[0].mpc.raw(), 0);
    assert_eq!(trace[1].mpc.raw(), 1);
    assert_eq!(trace[2].l.raw(), 1);
    assert_eq!(trace[7].l.raw(), 1);
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
    // Cycle 0: at addr 0; bus = 0 → F2 sets bit 0; next = 3.
    // Cycle 1: at addr 3 (loop forever).
    assert_eq!(trace[0].mpc.raw(), 0);
    assert_eq!(trace[1].mpc.raw(), 3, "BusEqZero should branch to addr 3");
    assert_eq!(trace[2].mpc.raw(), 3, "should loop at addr 3");
    assert_eq!(trace[1].l.raw(), 1, "L should latch 1 from cycle 0's ALU");
}
