//! F2 NEXT-modifier pipeline-timing regression test.
//!
//! Per AltoHW §2.4 (cross-checked with the standard microcode and
//! ContrAlto's `Tasks/Task.cs` lines 173-174 + 549), F2 NEXT
//! modifications are **delayed by exactly one cycle** in the real
//! Alto hardware:
//!
//! - At end of cycle K: F2 modifier from cycle K is latched.
//! - At end of cycle K+1: latched modifier is OR'd into cycle K+1's
//!   NEXT field, producing cycle K+2's MPC.
//!
//! Our microengine currently applies F2 NEXT modifications
//! **immediately** (same cycle).  This is the lockstep divergence
//! root cause documented in CHANGELOG entry "F2 NEXT-modifier
//! timing: spec verification + bug localization".
//!
//! See `alto-processor-and-microcode-spec.md` §2.3 for the
//! disambiguation evidence (loose AltoHW prose + standard microcode
//! behavior + ContrAlto cross-check).
//!
//! The test below is the minimal scenario distilled from CTR cycles
//! 40-42 of the boot trace (MPC 0x153 → 0x154 → 0x131):
//!
//!   Cycle K   runs MPC=0x100 with F1=Constant (BUS=1) + F2=BusToNext.
//!             Modifier latched = 1.  next field = 0x102 (bit 0 = 0).
//!   Cycle K+1 runs MPC=0x102 (DELAYED) or MPC=0x103 (IMMEDIATE).
//!             Latched modifier from K (= 1) applies HERE per delayed.
//!             0x102's next field = 0x108.  Modified by 1 → 0x109.
//!   Cycle K+2 runs MPC=0x109 (DELAYED) or MPC=0x103 (IMMEDIATE).
//!
//! The test asserts the DELAYED sequence — currently failing because
//! our impl produces the IMMEDIATE sequence.  Marked `#[ignore]`
//! until the F2-NEXT-modifier-timing fix lands.

use rhdl::prelude::*;
use rhdl_alto::alto_chip::{AltoChip, ChipIn, ChipOut};
use rhdl_alto::isa::{AluFunction, BusSource, F1Function, F2Function, Microinstruction};

const NUM_CONSTANTS: usize = rhdl_alto::constant_rom::NUM_CONSTANTS;
const MICROCODE_WORDS: usize = rhdl_alto::microcode_rom::MICROCODE_WORDS;

fn b5(v: u128) -> Bits<5> {
    bits::<5>(v)
}
fn b10(v: u128) -> Bits<10> {
    bits::<10>(v)
}

fn build_microcode_and_constants() -> ([u32; MICROCODE_WORDS], [u16; NUM_CONSTANTS]) {
    let mut microcode = [0u32; MICROCODE_WORDS];
    let mut constants = [0u16; NUM_CONSTANTS];

    // Constant ROM at index (RSEL=0, BS=ReadR=0) → constants[0] = 1.
    // F1=Constant at MPC=0x100 below will gate this onto BUS.
    constants[0] = 0x0001;

    // MPC=0x000 (Emulator reset, NOVEM): jump straight to the test.
    microcode[0] = Microinstruction {
        rsel: b5(0),
        aluf: AluFunction::Bus,
        bs: BusSource::ReadR,
        f1: F1Function::Nop,
        f2: F2Function::Nop,
        t_load: false,
        l_load: false,
        next: b10(0x100),
    }
    .pack();

    // MPC=0x100: F1=Constant (BUS=constants[(0<<3)|0]=1) + F2=BusToNext.
    //   Modifier = BUS & 0x3FF = 1.
    //   next field = 0x102 (bit 0 = 0).
    //   - DELAYED: cycle K+1 starts at 0x102, modifier latched.
    //   - IMMEDIATE: cycle K+1 starts at 0x102 | 1 = 0x103.
    microcode[0x100] = Microinstruction {
        rsel: b5(0),
        aluf: AluFunction::Bus,
        bs: BusSource::ReadR,
        f1: F1Function::Constant,
        f2: F2Function::BusToNext,
        t_load: false,
        l_load: false,
        next: b10(0x102),
    }
    .pack();

    // MPC=0x102: plain instruction, next=0x108.  Reached only in DELAYED.
    //   Cycle K+1 here applies the modifier from K → next = 0x108 | 1 = 0x109.
    microcode[0x102] = Microinstruction {
        rsel: b5(0),
        aluf: AluFunction::Bus,
        bs: BusSource::ReadR,
        f1: F1Function::Nop,
        f2: F2Function::Nop,
        t_load: false,
        l_load: false,
        next: b10(0x108),
    }
    .pack();

    // MPC=0x103: trap-loop.  Reached only in IMMEDIATE; we'd loop here
    //   forever in the buggy impl.  Made into a self-loop so the trace
    //   makes the divergence visible without crashing.
    microcode[0x103] = Microinstruction {
        next: b10(0x103),
        ..Default::default()
    }
    .pack();

    // MPC=0x108: trap-loop.  Reached only in DELAYED if NEXT modifier is
    //   not applied to MPC=0x102's NEXT.  We don't expect to land here
    //   in either correct impl OR buggy impl — included for diagnostic.
    microcode[0x108] = Microinstruction {
        next: b10(0x108),
        ..Default::default()
    }
    .pack();

    // MPC=0x109: success-loop.  Reached in DELAYED (the correct path).
    microcode[0x109] = Microinstruction {
        next: b10(0x109),
        ..Default::default()
    }
    .pack();

    (microcode, constants)
}

fn run(uut: AltoChip, cycles: usize) -> Vec<ChipOut> {
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
        .collect()
}

/// **Regression test.**  Pins the F2 NEXT-modifier delayed-pipeline
/// semantics per spec digest §2.3.  Originally written as a
/// `#[ignore]`-tagged failing test in the prior PR; now passing
/// after the F2-NEXT-modifier-timing fix in this PR.
///
/// The minimal scenario is distilled from the boot-trace divergence at
/// CTR cycles 40-42 (MPC 0x153 → 0x154 → 0x131): F2=BusToNext at one
/// MPC must apply its modifier to the NEXT instruction's NEXT field,
/// not its own.  See `alto-processor-and-microcode-spec.md` §2.3.
#[test]
fn f2_bus_to_next_is_applied_one_cycle_later_per_spec_2_3() {
    let (microcode, constants) = build_microcode_and_constants();
    let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
    let trace = run(uut, 6);

    eprintln!("MPC trace (first 6 cycles):");
    for (i, t) in trace.iter().enumerate() {
        eprintln!("  cycle {i}: mpc=0x{:03x}", t.mpc.raw());
    }

    // Cycle 0: NOVEM at MPC=0, jumps to 0x100.
    assert_eq!(
        trace[0].mpc.raw(),
        0x000,
        "cycle 0 should run MPC=0x000 (Emulator reset)"
    );

    // Cycle 1: runs MPC=0x100, computes F2=BusToNext modifier=1.
    //   DELAYED: modifier latched, NOT applied to 0x100's NEXT.
    //            cycle 2 starts at 0x102 (unmodified next field).
    //   IMMEDIATE (current bug): modifier applied to 0x100's NEXT.
    //            cycle 2 starts at 0x102 | 1 = 0x103.
    assert_eq!(
        trace[1].mpc.raw(),
        0x100,
        "cycle 1 should run MPC=0x100 (the BusToNext instruction)"
    );

    // Cycle 2: under DELAYED semantics, runs MPC=0x102.
    assert_eq!(
        trace[2].mpc.raw(),
        0x102,
        "cycle 2 SHOULD run MPC=0x102 per delayed F2-NEXT pipeline \
         (AltoHW §2.4 + spec digest §2.3 + ContrAlto Task.cs:173,549). \
         Currently FAILS (buggy impl produces 0x103 = 0x102 | 1 — the \
         immediate-apply behavior)."
    );

    // Cycle 3: under DELAYED semantics, runs MPC=0x109 (= 0x108 | 1).
    //   The modifier latched at cycle 1 is now applied to cycle 2's
    //   NEXT (= 0x108), producing MPC=0x109 for cycle 3.
    assert_eq!(
        trace[3].mpc.raw(),
        0x109,
        "cycle 3 SHOULD run MPC=0x109 — the F2=BusToNext modifier from \
         cycle 1 (= 1) applied to cycle 2's NEXT field (= 0x108).  \
         Currently FAILS (buggy impl loops at 0x103)."
    );

    // Negative assertions: prove we're not in the buggy MPC stream.
    assert_ne!(
        trace[2].mpc.raw(),
        0x103,
        "cycle 2 must NOT be 0x103 — that's the IMMEDIATE-apply \
         behavior, which is the bug being fixed"
    );
    assert_ne!(
        trace[3].mpc.raw(),
        0x103,
        "cycle 3 must NOT be 0x103 either"
    );
}

/// **Multi-cycle ALUCY + delayed-pipeline regression test.**
///
/// Exercises the interaction between (a) the sticky alu_carry DFF
/// (latched on L-load per spec §3.4 footnote) and (b) the delayed
/// F2-NEXT-modifier pipeline (spec digest §2.3).  Uses the chip-level
/// composition because the standalone-microengine simulator's
/// combinational-settle artifact masks the cycle boundary (see spec
/// digest §2.3 "Test-writing note" for details).
///
/// Microcode shape:
///   MPC=0x000: ALU=BusPlusOne(BUS=R[0]=0xFFFF after F1=Constant),
///              l_load=true.  ALU result = 0; aout.carry = 1.
///              At cycle edge: d.alu_carry ← 1.  next field = 0x010.
///   MPC=0x010: F2=AluCarryToNext.  q.alu_carry = true (from prev).
///              Modifier = 1 latched.  Per delayed pipeline, applied
///              to NEXT cycle's NEXT field (NOT this cycle's).
///              next field = 0x020 (bit 0 = 0).
///              cycle's next_mpc = 0x020 | q.next_modifier_pending(=0)
///              = 0x020.
///   MPC=0x020: q.next_modifier_pending = 1 (from prev cycle).
///              next field = 0x040 (bit 0 = 0).
///              next_mpc = 0x040 | 1 = 0x041.
///   MPC=0x041: target — distinguishable from immediate-apply.
///
/// Under DELAYED (correct) pipeline:
///   trace[1].mpc = 0x000 (start)
///   trace[2].mpc = 0x010
///   trace[3].mpc = 0x020 (NOT 0x021 — modifier deferred)
///   trace[4].mpc = 0x041 (= 0x040 | 1, modifier from cycle 2 applied)
///
/// Under IMMEDIATE (buggy) pipeline:
///   trace[3].mpc = 0x021 (modifier applied immediately)
///   trace[4].mpc = 0x041 OR something else (depends on chain)
#[test]
fn alucy_with_sticky_carry_uses_delayed_modifier_chip_level() {
    let mut microcode = [0u32; MICROCODE_WORDS];
    let mut constants = [0u16; NUM_CONSTANTS];
    // Constant ROM at (RSEL=0, BS=ReadR=0) → constants[0] = 0xFFFF.
    // F1=Constant gates this onto BUS at MPC=0.
    constants[0] = 0xFFFF;

    // MPC=0x000: F1=Constant (BUS=0xFFFF), aluf=BusPlusOne →
    //   ALU=0, carry=1.  l_load=true → d.alu_carry ← 1.
    //   No F2 modifier.  next field = 0x010.
    microcode[0] = Microinstruction {
        rsel: b5(0),
        aluf: AluFunction::BusPlusOne,
        bs: BusSource::ReadR,
        f1: F1Function::Constant,
        f2: F2Function::Nop,
        t_load: false,
        l_load: true,
        next: b10(0x010),
    }
    .pack();

    // MPC=0x010: F2=AluCarryToNext.  q.alu_carry = true (from prev).
    //   Modifier = 1 (latched).  next field = 0x020.
    //   - DELAYED: cycle 3 starts at 0x020 (no modifier applied here).
    //   - IMMEDIATE: cycle 3 starts at 0x020 | 1 = 0x021.
    microcode[0x010] = Microinstruction {
        rsel: b5(0),
        aluf: AluFunction::Bus,
        bs: BusSource::ReadR,
        f1: F1Function::Nop,
        f2: F2Function::AluCarryToNext,
        t_load: false,
        l_load: false,
        next: b10(0x020),
    }
    .pack();

    // MPC=0x020: NO F2 modifier.  next field = 0x040.
    //   DELAYED: q.next_modifier_pending = 1 (from cycle 2's ALUCY) →
    //            next_mpc = 0x040 | 1 = 0x041.
    //   IMMEDIATE wouldn't reach this MPC anyway.
    microcode[0x020] = Microinstruction {
        rsel: b5(0),
        aluf: AluFunction::Bus,
        bs: BusSource::ReadR,
        f1: F1Function::Nop,
        f2: F2Function::Nop,
        t_load: false,
        l_load: false,
        next: b10(0x040),
    }
    .pack();

    // MPC=0x041: target loop.
    microcode[0x041] = Microinstruction {
        next: b10(0x041),
        ..Default::default()
    }
    .pack();

    // MPC=0x021: trap-loop for IMMEDIATE-buggy detection.
    microcode[0x021] = Microinstruction {
        next: b10(0x021),
        ..Default::default()
    }
    .pack();

    let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
    let trace = run(uut, 6);

    eprintln!("ALUCY trace (first 6 cycles):");
    for (i, t) in trace.iter().enumerate() {
        eprintln!("  cycle {i}: mpc=0x{:03x}", t.mpc.raw());
    }

    // Cycle 0: runs MPC=0x000 (constant + BusPlusOne, latches alu_carry).
    assert_eq!(trace[0].mpc.raw(), 0x000, "cycle 0 should run MPC=0x000");
    // Cycle 1: runs MPC=0x010 (ALUCY).
    assert_eq!(
        trace[1].mpc.raw(),
        0x010,
        "cycle 1 should run MPC=0x010 (the ALUCY instruction)"
    );
    // Cycle 2: under DELAYED, runs MPC=0x020 (modifier latched, not applied here).
    assert_eq!(
        trace[2].mpc.raw(),
        0x020,
        "cycle 2 SHOULD run MPC=0x020 — ALUCY modifier from cycle 1 \
         is latched but NOT applied to cycle 1's NEXT (delayed pipeline). \
         If this fails with 0x021, the F2-NEXT-modifier-timing fix has \
         regressed (immediate-apply behavior)."
    );
    // Cycle 3: ALUCY's modifier (= 1) now applies to cycle 2's NEXT
    //   (= 0x040).  next_mpc = 0x041.
    assert_eq!(
        trace[3].mpc.raw(),
        0x041,
        "cycle 3 SHOULD run MPC=0x041 — ALUCY modifier from cycle 1 \
         applied to cycle 2's NEXT (= 0x040 | 1).  Confirms BOTH the \
         sticky carry (alu_carry latched on cycle 0's L-load is \
         visible to ALUCY at cycle 1) AND the delayed pipeline \
         (modifier from cycle 1 lands on cycle 2's NEXT)."
    );

    // Negative assertion: cycle 2 must NOT be 0x021 (immediate-apply).
    assert_ne!(
        trace[2].mpc.raw(),
        0x021,
        "cycle 2 must NOT be 0x021 — that's the IMMEDIATE-apply \
         behavior (the F2-NEXT-modifier-timing bug)"
    );
}
