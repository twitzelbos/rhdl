//! Comprehensive per-spec-rule microinstruction semantics tests.
//!
//! Each test isolates ONE spec rule (from `alto-processor-and-microcode-
//! spec.md` and the canonical `Alto_Hardware_Manual_Aug76.pdf`) and
//! verifies the chip implements it exactly.  This catches silent
//! semantic bugs — like the R-from-L vs R-from-ALU bug discovered in
//! commit `8253323a` (spec §2.7).
//!
//! Coverage map (each section = one spec rule family):
//! - §2.7  R-register file semantics (R writes from Shifter Output)
//! - §3.1  AluFunction (16 codes, sample-coverage of representatives)
//! - §3.2  BusSource (8 codes; universal + per-task variants)
//! - §3.3  F1Function — universal (0-7) and per-task (8-15)
//! - §3.4  F2Function — universal (0-7) and per-task (8-15)
//! - §3.5  IDISP PROM 16-way dispatch
//! - §4.4  Memory MAR / MD timing
//! - §5.4  TaskYield → engine.task_yield output
//! - §5.5  Block → engine.block_task output
//! - §6.6  ACDEST / ACSOURCE / BUSODD / STARTF (Emulator F2/F1)
//! - §8.5  Per-task F1/F2/BS Disk codes
//!
//! Sources:
//! - Authoritative: `crates/rhdl-alto/alto-processor-and-microcode-spec.md`.
//! - Spec-ambiguous: ContrAlto's `Task.cs` and `CPU.cs`.

use rhdl::core::sim::ResetOrData;
use rhdl::prelude::*;
use rhdl_alto::isa::{AluFunction, BusSource, F1Function, F2Function, Microinstruction};
use rhdl_alto::microengine::{In, Microengine, Out};

#[derive(Clone, Copy)]
struct InCfg {
    instr: u32,
    constant: u16,
    task: u8,
    md: u16,
    kdata: u16,
    kstat: u16,
}
impl InCfg {
    fn new(instr: u32, constant: u16) -> Self {
        Self { instr, constant, task: 0, md: 0, kdata: 0, kstat: 0 }
    }
    fn with_task(mut self, t: u8) -> Self { self.task = t; self }
    fn with_md(mut self, md: u16) -> Self { self.md = md; self }
    fn with_kdata(mut self, k: u16) -> Self { self.kdata = k; self }
    fn with_kstat(mut self, k: u16) -> Self { self.kstat = k; self }
}

/// Output split into the two timings that matter:
/// - `comb`: output computed during the LAST action's cycle — correct
///   for combinational outputs (next_mpc, task_yield, block_task,
///   startf, disk_strobe, disk_clr_stat, disk_ctrl_*, mem_write_*,
///   mem_address, mem_write_data).
/// - `after`: output one cycle later (NOP appended) — correct for
///   REGISTERED outputs (t, l, ir, mar) that latch on the action's edge.
struct Observation {
    comb: Out,
    after: Out,
}

/// Run `prog` followed by a NOP cycle, capturing every cycle's output
/// so we can return both the action's combinational output and the
/// post-edge registered state.
fn observe(prog: Vec<InCfg>) -> Observation {
    let nop_instr = ui(0, AluFunction::Bus, BusSource::ReadR,
        F1Function::Nop, F2Function::Nop, false, false, 0);
    let task = prog.last().map(|c| c.task).unwrap_or(0);
    let mut full = prog;
    full.push(InCfg::new(nop_instr, 0).with_task(task));

    let uut = Microengine::default();
    let mut cur = 0usize;
    let mut reset_remaining = 2usize;
    let mut outs: Vec<Out> = Vec::new();
    uut.run_fn(
        |out: Out| {
            if reset_remaining > 0 {
                reset_remaining -= 1;
                return Some(ResetOrData::Reset);
            }
            outs.push(out);
            if cur >= full.len() {
                return None;
            }
            let i = full[cur];
            cur += 1;
            Some(ResetOrData::Data(In {
                mpc: bits::<10>(0),
                instr: bits::<32>(i.instr as u128),
                constant_value: bits::<16>(i.constant as u128),
                mem_read_data: bits::<16>(i.md as u128),
                current_task: bits::<4>(i.task as u128),
                disk_word_data: bits::<16>(0),
                kcwa: bits::<16>(0),
                kstat: bits::<16>(i.kstat as u128),
                kdata: bits::<16>(i.kdata as u128),
            }))
        },
        100,
    )
    .for_each(drop);
    // outs sequence (post-reset):
    //   outs[0]    = pre-input state (callback invocation 1, before any
    //                input pushed).
    //   outs[k]    = output produced AFTER step k's input was applied,
    //                for k = 1..=full.len().
    // Last action is full[full.len() - 2] (NOP is the last entry); its
    // step number is full.len() - 1.  So:
    //   comb  (output during last action's cycle)  = outs[full.len() - 1].
    //   after (output during the appended NOP cycle) = outs[full.len()].
    let n = full.len();
    let comb = outs[n - 1].clone();
    let after = outs[n].clone();
    Observation { comb, after }
}

/// REGISTERED-output convenience: returns the post-edge view.
fn observe_after(prog: Vec<InCfg>) -> Out {
    observe(prog).after
}

/// COMBINATIONAL-output convenience: returns the action-cycle view.
fn observe_comb(prog: Vec<InCfg>) -> Out {
    observe(prog).comb
}

fn ui(rsel: u8, aluf: AluFunction, bs: BusSource, f1: F1Function, f2: F2Function,
      t_load: bool, l_load: bool, next: u16) -> u32 {
    Microinstruction {
        rsel: bits::<5>(rsel as u128),
        aluf, bs, f1, f2, t_load, l_load,
        next: bits::<10>(next as u128),
    }.pack()
}

// =====================================================================
// §2.7 — R-register file semantics: R ← Shifter Output (L), not ALU
// =====================================================================

#[test]
fn r_write_takes_l_value_not_alu() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0xCAFE),
        InCfg::new(ui(5, AluFunction::Bus, BusSource::LoadR,
            F1Function::Nop, F2Function::Nop, false, false, 0), 0),
        InCfg::new(ui(5, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0),
    ]);
    assert_eq!(out.t.raw() as u16, 0xCAFE,
        "R[5] must hold L's value (0xCAFE), not ALU=0");
}

#[test]
fn r_write_with_lsh1_takes_shifted_l() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x0042),
        InCfg::new(ui(3, AluFunction::Bus, BusSource::LoadR,
            F1Function::LeftShift1, F2Function::Nop, false, false, 0), 0),
        InCfg::new(ui(3, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0),
    ]);
    assert_eq!(out.t.raw() as u16, 0x0084,
        "R[3] should be L<<1 = 0x0042<<1 = 0x0084 (shifter output)");
}

#[test]
fn r_write_disabled_when_bs_not_loadr() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0xDEAD),
        InCfg::new(ui(5, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Nop, false, false, 0), 0),
        InCfg::new(ui(5, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0),
    ]);
    assert_eq!(out.t.raw() as u16, 0,
        "R[5] should remain 0 (BS=ReadR doesn't write)");
}

// =====================================================================
// §3.1 — ALU functions
// =====================================================================

#[test]
fn aluf_bus_passthrough() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x1234),
    ]);
    assert_eq!(out.l.raw() as u16, 0x1234);
}

#[test]
fn aluf_bus_plus_t() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x10),
        InCfg::new(ui(0, AluFunction::BusPlusT, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 5),
    ]);
    assert_eq!(out.l.raw() as u16, 0x15, "BusPlusT = 5+0x10");
}

#[test]
fn aluf_bus_minus_t() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 5),
        InCfg::new(ui(0, AluFunction::BusMinusT, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x10),
    ]);
    assert_eq!(out.l.raw() as u16, 0x0B, "BusMinusT = 0x10-5");
}

#[test]
fn aluf_bus_plus_one() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::BusPlusOne, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x4567),
    ]);
    assert_eq!(out.l.raw() as u16, 0x4568);
}

#[test]
fn aluf_bus_minus_one() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::BusMinusOne, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x100),
    ]);
    assert_eq!(out.l.raw() as u16, 0xFF);
}

#[test]
fn aluf_bus_or_t() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0xFF00),
        InCfg::new(ui(0, AluFunction::BusOrT, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x00FF),
    ]);
    assert_eq!(out.l.raw() as u16, 0xFFFF);
}

#[test]
fn aluf_bus_and_t() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0xF0F0),
        InCfg::new(ui(0, AluFunction::BusAndT, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0xFF00),
    ]);
    assert_eq!(out.l.raw() as u16, 0xF000);
}

#[test]
fn aluf_bus_xor_t() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x5555),
        InCfg::new(ui(0, AluFunction::BusXorT, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0xFFFF),
    ]);
    assert_eq!(out.l.raw() as u16, 0xAAAA);
}

#[test]
fn aluf_bus_and_not_t() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x00FF),
        InCfg::new(ui(0, AluFunction::BusAndNotT, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0xFFFF),
    ]);
    assert_eq!(out.l.raw() as u16, 0xFF00);
}

// =====================================================================
// §3.2 — Bus sources
// =====================================================================

#[test]
fn bs_read_r_drives_bus_from_r() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x789A),
        InCfg::new(ui(7, AluFunction::Bus, BusSource::LoadR,
            F1Function::Nop, F2Function::Nop, false, false, 0), 0),
        InCfg::new(ui(7, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0),
    ]);
    assert_eq!(out.t.raw() as u16, 0x789A);
}

#[test]
fn bs_load_r_forces_bus_zero() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x42),
        InCfg::new(ui(0, AluFunction::BusPlusT, BusSource::LoadR,
            F1Function::Nop, F2Function::Nop, false, true, 0), 0xDEAD),
    ]);
    assert_eq!(out.l.raw() as u16, 0x42,
        "BS=LoadR forces BUS=0; ALU=BusPlusT = 0+T = 0x42");
}

#[test]
fn bs_memory_data_drives_bus_from_md() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0).with_md(0xBEEF),
    ]);
    assert_eq!(out.t.raw() as u16, 0xBEEF);
}

#[test]
fn bs_taskspec3_reads_kstat_in_disk_sector() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::TaskSpec3,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0)
            .with_task(4).with_kstat(0xABCD),
    ]);
    assert_eq!(out.t.raw() as u16, 0xABCD);
}

#[test]
fn bs_taskspec3_reads_kstat_in_disk_word() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::TaskSpec3,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0)
            .with_task(14).with_kstat(0x1234),
    ]);
    assert_eq!(out.t.raw() as u16, 0x1234);
}

#[test]
fn bs_taskspec4_reads_kdata_in_disk_sector() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::TaskSpec4,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0)
            .with_task(4).with_kdata(0x5678),
    ]);
    assert_eq!(out.t.raw() as u16, 0x5678);
}

#[test]
fn bs_taskspec_returns_zero_in_emulator() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::TaskSpec3,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0)
            .with_task(0).with_kstat(0xFFFF),
    ]);
    assert_eq!(out.t.raw() as u16, 0,
        "BS=3 in Emulator (S-reg, not yet implemented) returns 0");
}

#[test]
fn bs_instruction_register_reads_ir_disp_field_not_full() {
    // CORRECTED: per spec §3.2 + ContrAlto's Task.cs ReadDisp handler,
    // BS=7 (`←DISP`) returns ONLY IR's low 8 bits (the displacement
    // field), NOT the full 16-bit IR.  Earlier this test asserted the
    // full-IR value and passed because the implementation had the same
    // bug — both wrong, test "validated" the wrong behavior.  Lockstep
    // against ContrAlto exposed the discrepancy via spec §3.2 BS=7
    // table entry (`Low-order 8 bits of IR, sign extended`).
    //
    // Pick IR with a NON-ZERO high byte so the bug would manifest:
    // IR=0x0100, X-field=1 (page-0 addressing), DISP=0x00 → BUS=0x00.
    // With X=0 (page-0): no sign extension → BUS = IR & 0xFF = 0.
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x0100).with_task(0),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::InstructionRegister,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0).with_task(0),
    ]);
    assert_eq!(out.t.raw() as u16, 0x0000,
        "BS=7 returns IR & 0xFF (DISP field) per spec §3.2; \
         IR=0x0100 → DISP=0x00, X-field=0 (no sign-ext) → BUS=0x0000");
}

#[test]
fn bs_disp_returns_low_byte_unextended_for_page_zero() {
    // X-field = IR bits 9-8 (Alto MSB=0 IR[6-7]) = 0 → page-0 addressing
    // → no sign extension regardless of DISP sign.  IR=0x0080 has DISP
    // bit 7 set, X=0 → BUS = 0x0080 (no sign extension).
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x0080).with_task(0),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::InstructionRegister,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0).with_task(0),
    ]);
    assert_eq!(out.t.raw() as u16, 0x0080,
        "BS=7 with X=0 (page-0 addressing) returns DISP zero-extended \
         even when sign bit is set");
}

#[test]
fn bs_disp_sign_extends_when_x_field_nonzero_and_sign_bit_set() {
    // X-field != 0 (PC-relative or base-register) AND sign bit (DISP[7])
    // set → sign-extend.  IR = 0x0180 (X=1, DISP=0x80 with sign bit set)
    // → BUS = 0xFF80.
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x0180).with_task(0),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::InstructionRegister,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0).with_task(0),
    ]);
    assert_eq!(out.t.raw() as u16, 0xFF80,
        "BS=7 with X!=0 and DISP sign bit set sign-extends to 0xFF80");
}

#[test]
fn bs_disp_no_sign_ext_when_x_nonzero_but_sign_bit_clear() {
    // X != 0 but sign bit (DISP[7]) clear → no sign extension.
    // IR = 0x0140 (X=1, DISP=0x40, sign bit clear) → BUS = 0x0040.
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x0140).with_task(0),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::InstructionRegister,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0).with_task(0),
    ]);
    assert_eq!(out.t.raw() as u16, 0x0040,
        "BS=7 with X!=0 but sign bit clear returns DISP zero-extended");
}

// =====================================================================
// §3.3 — F1 functions universal (0-7)
// =====================================================================

#[test]
fn f1_nop_does_nothing() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Nop, false, false, 0), 0),
    ]);
    assert_eq!(out.t.raw(), 0);
    assert_eq!(out.l.raw(), 0);
    assert!(!out.task_yield);
    assert!(!out.block_task);
}

#[test]
fn f1_load_mar_loads_at_edge() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::LoadMar, F2Function::Constant, false, false, 0), 0x9876),
    ]);
    assert_eq!(out.mem_address.raw() as u16, 0x9876);
}

#[test]
fn f1_task_yield_asserts() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::TaskYield, F2Function::Nop, false, false, 0), 0),
    ]);
    assert!(out.task_yield);
}

#[test]
fn f1_block_asserts() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Block, F2Function::Nop, false, false, 0), 0),
    ]);
    assert!(out.block_task);
}

#[test]
fn f1_left_shift_1() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x0001),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::LeftShift1, F2Function::Nop, false, false, 0), 0),
    ]);
    assert_eq!(out.l.raw() as u16, 0x0002);
}

#[test]
fn f1_right_shift_1() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x0080),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::RightShift1, F2Function::Nop, false, false, 0), 0),
    ]);
    assert_eq!(out.l.raw() as u16, 0x0040);
}

#[test]
fn f1_left_cycle_8() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x1234),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::LeftCycle8, F2Function::Nop, false, false, 0), 0),
    ]);
    assert_eq!(out.l.raw() as u16, 0x3412);
}

#[test]
fn f1_constant_drives_bus() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x55AA),
    ]);
    assert_eq!(out.t.raw() as u16, 0x55AA);
}

// =====================================================================
// §3.3 — F1 per-task (Disk: STROBE, LoadKSTAT, CLRSTAT, ...)
// =====================================================================

#[test]
fn f1_strobe_in_disk_sector() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Code9, F2Function::Nop, false, false, 0), 0).with_task(4),
    ]);
    assert!(out.disk_strobe);
}

#[test]
fn f1_strobe_in_disk_word() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Code9, F2Function::Nop, false, false, 0), 0).with_task(14),
    ]);
    assert!(out.disk_strobe);
}

#[test]
fn f1_strobe_does_not_assert_in_emulator() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Code9, F2Function::Nop, false, false, 0), 0).with_task(0),
    ]);
    assert!(!out.disk_strobe);
}

#[test]
fn f1_clrstat_in_disk_sector() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::WriteKcwa, F2Function::Nop, false, false, 0), 0).with_task(4),
    ]);
    assert!(out.disk_clr_stat);
}

#[test]
fn f1_loadkstat_writes_kstat_register() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Code10, F2Function::Constant, false, false, 0), 0xAABB).with_task(4),
    ]);
    assert!(out.disk_ctrl_write_en);
    assert_eq!(out.disk_ctrl_addr.raw(), 0);
    assert_eq!(out.disk_ctrl_write_data.raw() as u16, 0xAABB);
}

#[test]
fn f1_loadkcomm_writes_kcom_register() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::WriteKcomm, F2Function::Constant, false, false, 0), 0x8000).with_task(4),
    ]);
    assert!(out.disk_ctrl_write_en);
    assert_eq!(out.disk_ctrl_addr.raw(), 2);
    assert_eq!(out.disk_ctrl_write_data.raw() as u16, 0x8000);
}

#[test]
fn f1_loadkadr_writes_kadr_register() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::WriteKadr, F2Function::Constant, false, false, 0), 0x0500).with_task(4),
    ]);
    assert!(out.disk_ctrl_write_en);
    assert_eq!(out.disk_ctrl_addr.raw(), 3);
}

#[test]
fn f1_loadkdata_writes_kdata_register() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::WriteKdata, F2Function::Constant, false, false, 0), 0x1111).with_task(4),
    ]);
    assert!(out.disk_ctrl_write_en);
    assert_eq!(out.disk_ctrl_addr.raw(), 1);
}

#[test]
fn f1_disk_writes_inactive_in_emulator() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::WriteKcomm, F2Function::Nop, false, false, 0), 0).with_task(0),
    ]);
    assert!(!out.disk_ctrl_write_en,
        "F1=13 in Emulator = LoadESRB (not LoadKCOMM); no disk_ctrl write");
}

#[test]
fn f1_startf_in_emulator() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::WriteKdata, F2Function::Nop, false, false, 0), 0).with_task(0),
    ]);
    assert!(out.startf);
}

#[test]
fn f1_startf_inactive_in_disk() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::WriteKdata, F2Function::Nop, false, false, 0), 0).with_task(4),
    ]);
    assert!(!out.startf);
}

// =====================================================================
// §3.4 — F2 functions universal (0-7)
// =====================================================================

#[test]
fn f2_nop_does_not_modify_next() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Nop, false, false, 0x123), 0),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x123);
}

#[test]
fn f2_bus_eq_zero_sets_bit_when_bus_zero() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::LoadR,
            F1Function::Nop, F2Function::BusEqZero, false, false, 0x100), 0),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x101);
}

#[test]
fn f2_bus_eq_zero_no_change_when_bus_nonzero() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::BusEqZero, false, false, 0x100), 0x42),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x100);
}

#[test]
fn f2_bus_to_next_ors_low_bus_bits() {
    // BS=MemoryData drives BUS=MD; LoadIr latches IR=BUS per spec §6.6.
    // 0x0007 has bits 15,10,9,8 all clear so D17's IR← NEXT-merge is 0.
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x0007).with_task(0),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::InstructionRegister,
            F1Function::Nop, F2Function::BusToNext, false, false, 0x100), 0).with_task(0),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x107);
}

#[test]
fn f2_alu_carry_to_next_sets_bit_on_carry() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::BusPlusOne, BusSource::ReadR,
            F1Function::Constant, F2Function::AluCarryToNext, false, true, 0x200), 0xFFFF),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x201);
}

#[test]
fn f2_constant_drives_bus_same_as_f1() {
    let out_f1 = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x1000),
    ]);
    let out_f2 = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Constant, true, false, 0), 0x1000),
    ]);
    assert_eq!(out_f1.t.raw(), out_f2.t.raw());
    assert_eq!(out_f1.t.raw() as u16, 0x1000);
}

#[test]
fn f2_storemd_asserts_mem_write_en() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::StoreMd, false, false, 0), 0),
    ]);
    assert!(out.mem_write_en);
}

// =====================================================================
// §3.4 — F2 per-task Emulator (BUSODD, ACDEST, ACSOURCE, LoadIR, IDISP)
// =====================================================================

#[test]
fn f2_busodd_in_emulator_ors_bus_lsb() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::DiskWordTransfer, false, false, 0x100), 1)
            .with_task(0),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x101);
}

#[test]
fn f2_loadir_in_emulator() {
    // Per spec §6.6: IR← latches BUS (typically driven from MD via
    // BS=MemoryData).  Earlier impl bypassed BUS and read MD directly;
    // that's now fixed (D17), so BS must drive MD onto BUS.
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0xCAFE).with_task(0),
    ]);
    assert_eq!(out.ir.raw() as u16, 0xCAFE);
}

#[test]
fn f2_loadir_inactive_in_disk_task() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0xDEAD).with_task(4),
    ]);
    assert_eq!(out.ir.raw() as u16, 0,
        "F2=12 in Disk task = SWRNRDY, not LoadIR");
}

#[test]
fn f2_idisp_uses_prom_dispatch() {
    // Use IR=0x4000 (still IR[1-2]=2 → dispatch=5) instead of 0xCAFE
    // so the LoadIr cycle's BUS bits don't trigger the D17 IR← NEXT
    // merge (which would dispatch the LoadIr cycle to a non-zero
    // address).  BS=MemoryData drives BUS = MD.
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x4000).with_task(0),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::IDispatch, false, false, 0x100), 0).with_task(0),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x105,
        "IDISP PROM: IR[1-2]=2 → dispatch=5; NEXT|5 = 0x105");
}

#[test]
fn f2_idisp_with_ir_zero() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::IDispatch, false, false, 0x200), 0).with_task(0),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x200);
}

#[test]
fn f2_idisp_does_not_apply_in_disk_task() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::IDispatch, false, false, 0x100), 0).with_task(4),
    ]);
    // Disk: F2=13 = NFER (NEXT |= 1), not IDISP.
    assert_eq!(out.next_mpc.raw() as u16, 0x101,
        "F2=13 in Disk = NFER, not IDISP");
}

// =====================================================================
// §3.4 — F2 per-task Disk (NFER, etc.)
// =====================================================================

#[test]
fn f2_nfer_in_disk_sets_bit_no_error() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::IDispatch, false, false, 0x100), 0).with_task(14),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x101);
}

// =====================================================================
// §4.4 — Memory MAR / MD timing
// =====================================================================

#[test]
fn mar_load_takes_effect_on_next_cycle() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::LoadMar, F2Function::Constant, false, false, 0), 0x1234),
    ]);
    assert_eq!(out.mem_address.raw() as u16, 0x1234);
}

#[test]
fn store_md_writes_at_mar_with_bus() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::LoadMar, F2Function::Constant, false, false, 0), 0x100),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::LoadR,
            F1Function::Nop, F2Function::StoreMd, false, false, 0), 0),
    ]);
    assert!(out.mem_write_en);
    assert_eq!(out.mem_address.raw() as u16, 0x100);
    assert_eq!(out.mem_write_data.raw() as u16, 0);
}

// =====================================================================
// §6.6 — ACDEST / ACSOURCE override RSEL low 2 bits from IR
// =====================================================================

#[test]
fn acdest_overrides_low_2_rsel_from_ir() {
    // IR = 0x1000 → bits 12-11 = 0b10 = 2.  ACDEST: low 2 bits = 2 XOR 3 = 1.
    // Pre-load R[1] = 0xABCD; then ACDEST with rsel=0 reads R[(0&~3)|1] = R[1].
    // 0x1000 has bits 15,10,9,8 all clear so D17's IR← NEXT-merge is 0.
    let out = observe_after(vec![
        // Cycle 0: load IR.  BS=MemoryData drives BUS = MD per spec §6.6.
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x1000).with_task(0),
        // Cycle 1: prep L = 0xABCD.
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0xABCD).with_task(0),
        // Cycle 2: write R[1] = L = 0xABCD.
        InCfg::new(ui(1, AluFunction::Bus, BusSource::LoadR,
            F1Function::Nop, F2Function::Nop, false, false, 0), 0).with_task(0),
        // Cycle 3: read via ACDEST with rsel=0 → effective RSEL = 0|1 = 1.
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Code11, true, false, 0), 0).with_task(0),
    ]);
    assert_eq!(out.t.raw() as u16, 0xABCD);
}

#[test]
fn acsource_overrides_low_2_rsel_from_ir_src() {
    // IR = 0x2000 → bits 14-13 = 0b01 = 1.  ACSOURCE: 1 XOR 3 = 2.
    // 0x2000 has bits 15,10,9,8 all clear so D17's IR← NEXT-merge is 0.
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x2000).with_task(0),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x9999).with_task(0),
        InCfg::new(ui(2, AluFunction::Bus, BusSource::LoadR,
            F1Function::Nop, F2Function::Nop, false, false, 0), 0).with_task(0),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Code14, true, false, 0), 0).with_task(0),
    ]);
    assert_eq!(out.t.raw() as u16, 0x9999);
}

// =====================================================================
// T_LOAD / L_LOAD interactions
// =====================================================================

#[test]
fn t_load_takes_bus_value() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x4242),
    ]);
    assert_eq!(out.t.raw() as u16, 0x4242);
}

#[test]
fn l_load_takes_alu_result() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::BusPlusOne, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 5),
    ]);
    assert_eq!(out.l.raw() as u16, 6);
}

#[test]
fn no_t_load_keeps_t() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x1111),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, false, 0), 0xFFFF),
    ]);
    assert_eq!(out.t.raw() as u16, 0x1111);
}

#[test]
fn no_l_load_keeps_l() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x1111),
        InCfg::new(ui(0, AluFunction::BusPlusOne, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, false, 0), 0xFFFF),
    ]);
    assert_eq!(out.l.raw() as u16, 0x1111);
}

// =====================================================================
// §3.1 — T←ALU asterisk semantics (the canonical accumulator pattern)
// =====================================================================
//
// Per spec §3.1 footnote: "If T is loaded during an instruction which
// specifies [an asterisked ALU function], it will be loaded from the
// ALU output rather than from the bus."  Asterisked ALUFs: 2 (BusOrT),
// 5 (BusPlusOne), 6 (BusMinusOne), 10 (BusPlusTPlusOne), 12 (BusAndTAlt).

#[test]
fn t_load_with_bus_or_t_loads_alu_result_not_bus() {
    // Pre-load T = 0x00FF.  Then `T← BUS OR T` with BUS = 0xFF00 must
    // load T from the ALU output (BUS|T = 0xFFFF), NOT from BUS (0xFF00).
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x00FF),
        InCfg::new(ui(0, AluFunction::BusOrT, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0xFF00),
    ]);
    assert_eq!(out.t.raw() as u16, 0xFFFF,
        "T← BUS OR T must load T from ALU output (BUS|T = 0xFFFF), \
         not from BUS (0xFF00).  This is the canonical accumulator \
         pattern per spec §3.1 footnote.");
}

#[test]
fn t_load_with_bus_plus_one_loads_alu_result() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::BusPlusOne, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x4242),
    ]);
    assert_eq!(out.t.raw() as u16, 0x4243,
        "T← BUS+1 must load T from ALU output (0x4243), not BUS (0x4242)");
}

#[test]
fn t_load_with_non_asterisked_aluf_loads_bus() {
    // ALUF=0 (Bus) is NOT asterisked; T← 0x1234 with this loads BUS.
    // (ALU result = BUS = 0x1234, so result coincidentally matches.)
    // Use ALUF=8 (BusMinusT) which IS NOT asterisked: T preloaded to
    // 0x10, BUS=0x100, ALU=BUS-T=0xF0; T should load from BUS=0x100.
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x10),
        InCfg::new(ui(0, AluFunction::BusMinusT, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x100),
    ]);
    assert_eq!(out.t.raw() as u16, 0x100,
        "T← with non-asterisked ALUF (BusMinusT) loads from BUS, \
         not ALU result.  BUS=0x100, T should be 0x100 not ALU=0xF0.");
}

// =====================================================================
// §3.2 — BusSource::None reads as -1 (wired-AND default)
// =====================================================================

#[test]
fn bs_none_reads_as_minus_one() {
    // Per spec §3.2: "Nothing — bus reads as -1 (all-ones, no source
    // asserting)" because the Alto bus is wired-AND.
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::None,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0).with_task(0),
    ]);
    assert_eq!(out.t.raw() as u16, 0xFFFF,
        "BS=None must read as 0xFFFF (-1), the wired-AND default per spec §3.2");
}

// =====================================================================
// §3.4 — F2=ALUCY uses sticky-from-last-L-load carry
// =====================================================================

#[test]
fn alucy_uses_sticky_carry_from_last_l_load() {
    // Spec §3.4 footnote: "the carry [used by F2=ALUCY] is that
    // produced by the ALU function which last loaded the L register."
    // Cycle 1: BUS=0xFFFF, ALU=BUS+1, L_LOAD=true → L = 0x0000, CARRY = 1.
    //   The sticky_carry register latches 1.
    // Cycle 2: BUS=1, ALU=BUS, L_LOAD=false (so sticky carry stays 1).
    //   F2=ALUCY should set NEXT bit 0 because LAST-L-LOAD's carry was 1,
    //   even though THIS cycle's ALU=BUS does not produce a carry.
    let out = observe_comb(vec![
        // Cycle 1: BUS+1 with BUS=0xFFFF carries; L latches 0; sticky carry = 1.
        InCfg::new(ui(0, AluFunction::BusPlusOne, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0xFFFF),
        // Cycle 2: BUS=0, ALU=BUS, no carry, but ALUCY uses sticky carry.
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::AluCarryToNext, false, false, 0x100), 0),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x101,
        "F2=ALUCY must use the carry from the LAST cycle that loaded L, \
         not this cycle's carry.  Spec §3.4 footnote.");
}

// =====================================================================
// §3.4 — F2=SH<0 sign convention (was inverted; D10 fix)
// =====================================================================

#[test]
fn shift_lt_zero_sets_bit_when_l_negative() {
    // Pre-load L = 0x8000 (MSB set, negative).  F2=ShiftLessThanZero
    // should SET NEXT bit 0 because Shifter Output is negative.
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x8000),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::ShiftLessThanZero, false, false, 0x100), 0),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x101,
        "SH<0 must set NEXT bit 0 when L's MSB is set (negative)");
}

#[test]
fn shift_lt_zero_clears_bit_when_l_non_negative() {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x7FFF),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::ShiftLessThanZero, false, false, 0x100), 0),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x100,
        "SH<0 must NOT set NEXT bit 0 when L's MSB is clear (non-negative)");
}

// =====================================================================
// §6.6 — IR← merges BUS bits 0,5,6,7 into NEXT[3..0] (D17)
// =====================================================================

#[test]
fn ir_load_merges_bus_bit_15_into_next_bit_3() {
    // BUS bit 15 (= Alto IR[0]) → NEXT bit 3.  Set ONLY bit 15.
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0x100), 0)
            .with_md(0x8000).with_task(0),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x108,
        "IR← merges BUS bit 15 into NEXT bit 3 per spec §6.6");
}

#[test]
fn ir_load_merges_bus_bits_8_9_10_into_next_bits_0_1_2() {
    // BUS bits 10..8 (= Alto IR[5..7]) → NEXT bits 2..0.  Set 0x0700.
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0x100), 0)
            .with_md(0x0700).with_task(0),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x107,
        "IR← merges BUS bits 10,9,8 into NEXT bits 2,1,0 per spec §6.6");
}

#[test]
fn ir_load_merge_inactive_in_disk_task() {
    // F2=LoadIr is Emulator-only; in disk task, no merge happens.
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0x100), 0)
            .with_md(0x8700).with_task(4),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x100,
        "IR← merge is Emulator-only; disk task should leave NEXT alone");
}

// =====================================================================
// §3.4 — F2=BUSODD (Emulator) — additional coverage
// =====================================================================

#[test]
fn f2_busodd_does_not_set_bit_when_bus_lsb_clear() {
    // BUSODD per spec §6.6: BUS[15] (= our LSB) is OR'd into NEXT[9]
    // (= our LSB).  When BUS LSB = 0, NEXT must be unchanged.
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::DiskWordTransfer, false, false, 0x100), 2)
            .with_task(0),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x100,
        "BUSODD with even BUS must leave NEXT unchanged");
}

// =====================================================================
// §6.6 / §3.5 — IDISP PROM full table coverage (D12)
// =====================================================================
//
// Helper to set IR via LoadIr (BS=MemoryData) then run IDISP with
// NEXT=0x100 in the same observation; returns next_mpc combinationally.
fn idisp_dispatch_for_ir(ir: u16) -> u16 {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(ir).with_task(0),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::IDispatch, false, false, 0x100), 0).with_task(0),
    ]);
    out.next_mpc.raw() as u16
}

#[test]
fn idisp_branch_ir0_set_uses_complement_of_sh_field() {
    // IR[0]=1 (bit 15 set) → dispatch = 3 - IR[8-9].
    // IR[8-9] = bits 7-6.  Examples:
    //   IR=0x8000 → bits 7-6 = 0b00 → 3-0 = 3 → next = 0x103
    //   IR=0x8040 → bits 7-6 = 0b01 → 3-1 = 2 → next = 0x102
    //   IR=0x8080 → bits 7-6 = 0b10 → 3-2 = 1 → next = 0x101
    //   IR=0x80C0 → bits 7-6 = 0b11 → 3-3 = 0 → next = 0x100
    assert_eq!(idisp_dispatch_for_ir(0x8000), 0x103, "IR[0]=1 IR[8-9]=0 → 3");
    assert_eq!(idisp_dispatch_for_ir(0x8040), 0x102, "IR[0]=1 IR[8-9]=1 → 2");
    assert_eq!(idisp_dispatch_for_ir(0x8080), 0x101, "IR[0]=1 IR[8-9]=2 → 1");
    assert_eq!(idisp_dispatch_for_ir(0x80C0), 0x100, "IR[0]=1 IR[8-9]=3 → 0");
}

#[test]
fn idisp_branch_ir12_zero_uses_ir34() {
    // IR[1-2]=0 → dispatch = IR[3-4] (our bits 12-11).
    // To hit this: IR[0]=0 (bit 15 clear) AND IR[1-2]=0 (bits 14-13 = 00).
    //   IR=0x0000 → IR[3-4]=0 → next=0x100
    //   IR=0x0800 → IR[3-4]=1 → next=0x101
    //   IR=0x1000 → IR[3-4]=2 → next=0x102
    //   IR=0x1800 → IR[3-4]=3 → next=0x103
    assert_eq!(idisp_dispatch_for_ir(0x0000), 0x100);
    assert_eq!(idisp_dispatch_for_ir(0x0800), 0x101);
    assert_eq!(idisp_dispatch_for_ir(0x1000), 0x102);
    assert_eq!(idisp_dispatch_for_ir(0x1800), 0x103);
}

#[test]
fn idisp_branch_ir12_one_dispatches_to_4() {
    // IR[1-2]=1 (bit 14=0, bit 13=1) → dispatch = 4.  IR=0x2000.
    assert_eq!(idisp_dispatch_for_ir(0x2000), 0x104);
}

#[test]
fn idisp_branch_ir12_two_dispatches_to_5() {
    // IR[1-2]=2 (bit 14=1, bit 13=0) → dispatch = 5.  IR=0x4000.
    assert_eq!(idisp_dispatch_for_ir(0x4000), 0x105);
}

#[test]
fn idisp_branch_ir47_zero_dispatches_to_1() {
    // IR[1-2]=3 falls through to IR[4-7]=0 branch.
    // IR[1-2]=3 (bits 14-13 = 11) AND IR[4-7]=0 (bits 11-8 = 0000).
    // IR = 0x6000 has bits 14-13=11, bits 11-8=0 → satisfies both.
    assert_eq!(idisp_dispatch_for_ir(0x6000), 0x101);
}

#[test]
fn idisp_branch_ir47_one_dispatches_to_0() {
    // IR[1-2]=3 AND IR[4-7]=1 (bits 11-8 = 0001).
    // IR = 0x6100 (bits 14-13=11, bits 11-8=0001).
    assert_eq!(idisp_dispatch_for_ir(0x6100), 0x100);
}

#[test]
fn idisp_branch_ir47_six_dispatches_to_14() {
    // IR[1-2]=3 AND IR[4-7]=6 (bits 11-8 = 0110).  CONVERT.
    // IR = 0x6600.
    assert_eq!(idisp_dispatch_for_ir(0x6600), 0x10E);
}

#[test]
fn idisp_branch_ir47_fourteen_dispatches_to_6() {
    // IR[1-2]=3 AND IR[4-7]=14 (bits 11-8 = 1110).
    // IR = 0x6E00.  Per ContrAlto's spec: dispatch = 6.
    assert_eq!(idisp_dispatch_for_ir(0x6E00), 0x106);
}

#[test]
fn idisp_branch_default_uses_ir47() {
    // IR[1-2]=3 AND IR[4-7] not in {0,1,6,14} → dispatch = IR[4-7].
    // IR = 0x6500 (bits 14-13=11, bits 11-8=5) → dispatch=5.
    assert_eq!(idisp_dispatch_for_ir(0x6500), 0x105);
    // IR = 0x6700 (bits 11-8=7) → dispatch=7.
    assert_eq!(idisp_dispatch_for_ir(0x6700), 0x107);
}

// =====================================================================
// §6.6 — ACSOURCE late dispatch (D13 fix)
// =====================================================================
//
// Helper: set IR via LoadIr, then run F2=ACSOURCE (Code14) with
// NEXT=0x100; return the resulting next_mpc.  ACSOURCE has TWO roles:
// the early RSEL-override is tested separately; this exercises the
// late NEXT-modify dispatch.
fn acsource_dispatch_for_ir(ir: u16) -> u16 {
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(ir).with_task(0),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Code14, false, false, 0x100), 0).with_task(0),
    ]);
    out.next_mpc.raw() as u16
}

#[test]
fn acsource_ir0_set_uses_complement_of_sh_field() {
    // IR[0]=1 (bit 15) → dispatch = 3 - IR[8-9] (bits 7-6).
    // IR=0x8000: bits 7-6 = 00 → 3-0 = 3 → next = 0x103.
    // IR=0x80C0: bits 7-6 = 11 → 3-3 = 0 → next = 0x100.
    assert_eq!(acsource_dispatch_for_ir(0x8000), 0x103);
    assert_eq!(acsource_dispatch_for_ir(0x80C0), 0x100);
}

#[test]
fn acsource_ir37_zero_dispatches_to_2_cycle() {
    // IR[0]=0, IR[1-2]=3 (so no IR[5] OR), IR[3-7]=0 → dispatch=2.
    // IR=0x6000: bits 14-13=11, bits 12-8=00000, bit 10=0 → next=0x102.
    assert_eq!(acsource_dispatch_for_ir(0x6000), 0x102);
}

#[test]
fn acsource_ir37_one_dispatches_to_5_ramtrap() {
    // IR[3-7]=1 → dispatch=5.  IR=0x6100 (bits 12-8=00001).
    assert_eq!(acsource_dispatch_for_ir(0x6100), 0x105);
}

#[test]
fn acsource_ir37_two_dispatches_to_3_nopar() {
    // IR[3-7]=2 → dispatch=3.  IR=0x6200.
    assert_eq!(acsource_dispatch_for_ir(0x6200), 0x103);
}

#[test]
fn acsource_ir37_default_dispatches_to_14_romtrap() {
    // IR[3-7]=5 (not in special table) → dispatch=14.  IR=0x6500.
    assert_eq!(acsource_dispatch_for_ir(0x6500), 0x10E);
}

#[test]
fn acsource_ir37_31_dispatches_to_15_swat() {
    // IR[3-7]=31 (=37B octal) → dispatch=15 (=17B octal).  IR=0x7F00.
    assert_eq!(acsource_dispatch_for_ir(0x7F00), 0x10F);
}

#[test]
fn acsource_ir37_14_dispatches_to_1_convert() {
    // IR[3-7]=14 (=16B octal) → dispatch=1 (CONVERT).  IR=0x6E00.
    assert_eq!(acsource_dispatch_for_ir(0x6E00), 0x101);
}

#[test]
fn acsource_ir12_not_3_ors_indirect_bit_into_dispatch() {
    // IR[5] (bit 10) overlaps with IR[3-7] (bits 12-8), so for the
    // OR to be observable, pick IR[3-7] whose dispatch has bit 0
    // CLEAR.  IR[3-7]=5 → default dispatch=14 (bit 0 clear).  With
    // IR[5]=1 and IR[1-2]=0 (not 3) → ind_bit=1 OR'd in → 14|1=15.
    // IR=0x0500 (bits 14-13=00, bits 12-8=00101=5, bit 10=1).
    assert_eq!(acsource_dispatch_for_ir(0x0500), 0x10F,
        "IR[1-2]!=3 OR's the indirect-bit IR[5] into the dispatch");
}

#[test]
fn acsource_ir12_eq_3_does_not_or_indirect_bit() {
    // When IR[1-2]=3, IR[5] is NOT OR'd in.  IR=0x6500 has bits 14-13=11
    // (= 3), bits 12-8=00101 (=5), bit 10=1.  Dispatch=14 (default for
    // IR[3-7]=5); ind_bit suppressed → next=0x100 | 14 = 0x10E.
    assert_eq!(acsource_dispatch_for_ir(0x6500), 0x10E,
        "IR[1-2]=3 suppresses the IR[5] indirect-bit OR");
}

// =====================================================================
// §6.6 — DNS (Do Nova Shift) — F2=Code10 in Emulator (D16 full)
// =====================================================================
//
// DNS implements Nova SHIFT instruction emulation with several
// simultaneous side-effects:
//   - Modifies LSH/RSH to do Nova-style 17-bit rotates with carry.
//   - Computes new Nova CARRY based on IR carry-control + ALU op.
//   - Sets SKIP based on IR low 3 bits (Nova SKP modes) + result.
//   - Suppresses R-write when IR bit 12 (= our bit 3) is set.
//   - Latches new CARRY when R-write is enabled.

#[test]
fn dns_lsh_rotates_carry_into_bit_0() {
    // Pre-load CARRY=1 by running DNS with carry-control=O (force 1).
    // Then DNS+LSH: bit 0 ← carry_in.  L=0x0042, carry=1 → result =
    // (0x0042<<1) | 1 = 0x0085.
    // IR: carry-control = 2 (O = force 1), bits 5-4 = 0b10.
    //     skip-mode = 0, bits 0-2 = 000.
    //     arith op (bits 8-10) = 0 (COM = unaffected).  Bits 0x20.
    let out = observe_after(vec![
        // First: load IR with carry-control=2, IR[12]=0 (R-write enabled).
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x0020).with_task(0),
        // L <- 0x0042 (set up the value to shift).
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x0042),
        // DNS + LSH.  LoadDNS = F2=Code10.  carry-control=2 → carry_in=1.
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::LeftShift1, F2Function::Code10, false, false, 0), 0),
    ]);
    assert_eq!(out.l.raw() as u16, 0x0085,
        "DNS LSH with carry_in=1: (0x0042<<1) | 1 = 0x0085");
}

#[test]
fn dns_rsh_rotates_carry_into_bit_15() {
    // Same as above but RSH.  IR carry-control=O → carry_in=1.
    // L=0x0080, RSH+DNS: bit 15 ← 1 → (0x0080>>1) | 0x8000 = 0x8040.
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x0020).with_task(0),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x0080),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::RightShift1, F2Function::Code10, false, false, 0), 0),
    ]);
    assert_eq!(out.l.raw() as u16, 0x8040,
        "DNS RSH with carry_in=1: (0x0080>>1) | 0x8000 = 0x8040");
}

#[test]
fn dns_carry_control_z_forces_zero_carry_in() {
    // IR carry-control=1 (Z = force 0).  L=0x0042, LSH+DNS → no carry
    // injection → 0x0084 (plain LSH result).
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x0010).with_task(0),  // bits 5-4 = 0b01 = Z
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x0042),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::LeftShift1, F2Function::Code10, false, false, 0), 0),
    ]);
    assert_eq!(out.l.raw() as u16, 0x0084, "DNS carry-control=Z forces carry_in=0");
}

#[test]
fn dns_skip_mode_skp_always_sets_skip() {
    // IR low 3 bits = 1 (SKP = always skip).
    // After DNS, SKIP DFF should be set.  Verify by next-cycle ALUF=BusPlusSkip.
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x0001).with_task(0),  // bits 0-2 = 001 = SKP
        // DNS with no-op shift (mi.f1=Nop) so result is just whatever L is.
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Code10, false, false, 0), 0).with_task(0),
        // Now ALUF=BusPlusSkip with BUS=0; should give 0+SKIP = 1 (since SKP).
        InCfg::new(ui(0, AluFunction::BusPlusSkip, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0).with_task(0),
    ]);
    assert_eq!(out.l.raw() as u16, 1,
        "DNS SKP mode sets SKIP latch; subsequent ALUF=BusPlusSkip with BUS=0 → 0+SKIP=1");
}

#[test]
fn dns_skip_mode_szr_sets_skip_when_result_zero() {
    // IR low 3 bits = 4 (SZR = skip if result zero).
    // L=0, DNS+Nop → result = 0 → SKIP set.
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x0004).with_task(0),  // bits 0-2 = 100 = SZR
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0).with_task(0),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Code10, false, false, 0), 0).with_task(0),
        InCfg::new(ui(0, AluFunction::BusPlusSkip, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0).with_task(0),
    ]);
    assert_eq!(out.l.raw() as u16, 1,
        "DNS SZR mode + result=0 → SKIP set; ALUF=BusPlusSkip with BUS=0 → 1");
}

#[test]
fn dns_skip_mode_szr_clears_skip_when_result_nonzero() {
    // SZR + L=nonzero → SKIP NOT set.
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x0004).with_task(0),  // SZR
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x42).with_task(0),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Code10, false, false, 0), 0).with_task(0),
        InCfg::new(ui(0, AluFunction::BusPlusSkip, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0).with_task(0),
    ]);
    assert_eq!(out.l.raw() as u16, 0,
        "DNS SZR + result!=0 → SKIP=0; ALUF=BusPlusSkip with BUS=0 → 0");
}

#[test]
fn dns_ir_bit_3_set_suppresses_r_write() {
    // IR bit 3 (= IR[12] in Alto MSB=0) suppresses R-write under DNS.
    // First load R[5] with a known value, then DNS+BS=LoadR with IR[12]=1
    // — R[5] should NOT change.
    let out = observe_after(vec![
        // Load R[5] = 0xCAFE via L (per spec §2.7).
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0xCAFE),
        InCfg::new(ui(5, AluFunction::Bus, BusSource::LoadR,
            F1Function::Nop, F2Function::Nop, false, false, 0), 0),
        // Set IR[12]=1 (suppress) and IR[3-4]=ensures effective_rsel=5.
        // IR[3-4] XOR 3 with bits 12-11 → for effective_rsel low 2 bits=1
        // (so |= 1 to RSEL high bits), IR bits 12-11 = 0b10 (XOR 3 → 0b01).
        // Set IR=0x1008: bits 12-11 = 0b10, bit 3 = 1 (suppress).
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x1008).with_task(0),
        // DNS+BS=LoadR.  With effective_rsel from IR[3-4], target would
        // be R[(5 high) | 1]=... wait RSEL is 5 bits; with high-3 and
        // low-2 from IR.  Skip the address calculation; just verify
        // R[5] unchanged.  Use rsel=5 (high 3=001, low 2=01) → IR
        // override gives R[(rsel & ~3) | (XOR'd low 2)]= R[4|1]=R[5].
        // Actually rsel=5 = 0b00101.  rsel_high = 0b00100 = 4.  IR[3-4]
        // bits 12-11 = 0b10 = 2; XOR 3 = 1.  So effective_rsel = 4|1 = 5.
        // So would-be write target IS R[5].  Verify it's NOT written.
        InCfg::new(ui(5, AluFunction::Bus, BusSource::LoadR,
            F1Function::Nop, F2Function::Code10, false, true, 0), 0xDEAD).with_task(0),
        // Read R[5] back via T-load.
        InCfg::new(ui(5, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0).with_task(0),
    ]);
    assert_eq!(out.t.raw() as u16, 0xCAFE,
        "DNS with IR[12]=1 (suppress) must NOT write R; R[5] stays 0xCAFE");
}
//
// Per spec §6.6 + ContrAlto's Shifter.cs commentary: SKIP is a one-bit
// latch in the Emulator.  Set by F2=LoadDNS (not yet implemented);
// cleared by F2=LoadIr.  Read by ALUF=11 (BUS+SKIP) — when SKIP is
// set, the ALU adds 1 to BUS (otherwise it passes BUS through),
// implementing Nova's "skip on condition" semantics.

#[test]
fn skip_latch_defaults_to_zero_after_reset() {
    // Use ALUF=BusPlusSkip with BUS=5.  After reset, SKIP=false → ALU
    // returns BUS = 5.
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::BusPlusSkip, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 5),
    ]);
    assert_eq!(out.l.raw() as u16, 5,
        "After reset, SKIP=false; ALUF=BusPlusSkip with BUS=5 → 5");
}

#[test]
fn ir_load_clears_skip_latch() {
    // Even if SKIP were somehow set, IR← (F2=LoadIr) clears it.  The
    // current SKIP setter (LoadDNS) isn't implemented, so this test
    // primarily verifies the IR← clear path doesn't change the
    // already-false SKIP — and that ALUF=BusPlusSkip still returns BUS.
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x4000).with_task(0),
        InCfg::new(ui(0, AluFunction::BusPlusSkip, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 7),
    ]);
    assert_eq!(out.l.raw() as u16, 7,
        "After IR← clears SKIP, ALUF=BusPlusSkip with BUS=7 → 7");
}

// =====================================================================
// §4.4 — Memory MAR/MD timing rules (D2/D3 coverage)
// =====================================================================
//
// Per spec §4.4(a): "A minimum of one microinstruction must intervene
// between the initiation of a memory reference [F1=LoadMar] and an
// `MD←` [F2=StoreMd] or `←MD` [BS=MemoryData]."
//
// Per spec §4.4(b): the processor SUSPENDS execution if MD is touched
// before memory is ready — but our chip doesn't model the suspend
// (Phase 3.5 simplification).  These tests therefore exercise the
// well-formed pattern (one-cycle gap) — verifying that microcode
// respecting the rule sees consistent values.  An ill-formed test
// (no intervening cycle) would silently get stale data with our
// model; we don't test that case until the suspend behavior is
// modeled (Phase 4 follow-up).

#[test]
fn memory_read_with_intervening_cycle_returns_correct_value() {
    // Spec §4.4(a) well-formed pattern:
    //   Cycle 1: F1=LoadMar, BUS=address (drives MAR)
    //   Cycle 2: NOP (memory in flight)
    //   Cycle 3: BS=MemoryData → BUS gets MD (memory[address])
    // observe_after captures BS=MemoryData read into T.
    let out = observe_after(vec![
        // Cycle 1: load MAR with address from constant (use index 0).
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::LoadMar, F2Function::Constant, false, false, 0), 0x0080),
        // Cycle 2: intervening NOP (per spec §4.4(a)).
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Nop, false, false, 0), 0),
        // Cycle 3: T <- MD = whatever memory holds at 0x0080.
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0)
            .with_md(0xDEAD),
    ]);
    // Note: observe helper drives mem_read_data each cycle directly
    // (synthetic memory), not through a Memory sub-circuit.  So we
    // verify that BS=MemoryData latches the helper-supplied md value.
    assert_eq!(out.t.raw() as u16, 0xDEAD,
        "T <- MD with proper intervening cycle latches the supplied MD");
}

#[test]
fn memory_write_with_intervening_cycle_emits_correct_signals() {
    // Spec §4.4 well-formed write pattern:
    //   Cycle 1: F1=LoadMar (MAR <- 0x0100)
    //   Cycle 2: NOP
    //   Cycle 3: F2=StoreMd, BUS = data → mem_write_en + correct addr/data
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::LoadMar, F2Function::Constant, false, false, 0), 0x0100),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Nop, false, false, 0), 0),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::StoreMd, false, false, 0), 0xBEEF),
    ]);
    assert!(out.mem_write_en,
        "F2=StoreMd with proper MAR setup must assert mem_write_en");
    assert_eq!(out.mem_address.raw() as u16, 0x0100,
        "mem_address must reflect MAR loaded 2 cycles ago");
    assert_eq!(out.mem_write_data.raw() as u16, 0xBEEF,
        "mem_write_data must reflect this cycle's BUS");
}

// =====================================================================
// §6.6 — F2=MAGIC modifies LSH/RSH for double-length shifts (D15)
// =====================================================================

#[test]
fn magic_left_shift_injects_t_msb_into_bit_0() {
    // Pre-load T=0x8000 (MSB set), L=0x0042.  Then LSH+MAGIC should
    // produce L' = (0x0042 << 1) | (T MSB → bit 0) = 0x0084 | 1 = 0x0085.
    let out = observe_after(vec![
        // T <- 0x8000
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x8000),
        // L <- 0x0042
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x0042),
        // LSH + MAGIC (F2=Code9)
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::LeftShift1, F2Function::Code9, false, false, 0), 0),
    ]);
    assert_eq!(out.l.raw() as u16, 0x0085,
        "MAGIC LSH: bit 0 = T's MSB = 1; (0x0042<<1) | 1 = 0x0085");
}

#[test]
fn magic_left_shift_t_msb_clear_no_injection() {
    // T=0x0001 (MSB clear).  LSH+MAGIC should produce just (L << 1).
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x0001),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x0042),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::LeftShift1, F2Function::Code9, false, false, 0), 0),
    ]);
    assert_eq!(out.l.raw() as u16, 0x0084,
        "MAGIC LSH with T MSB clear: bit 0 = 0; same as plain LSH");
}

#[test]
fn magic_right_shift_injects_t_lsb_into_bit_15() {
    // T=0x0001 (LSB set), L=0x0080.  RSH+MAGIC: L' = (L>>1) | (T LSB << 15)
    //   = 0x0040 | 0x8000 = 0x8040.
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x0001),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x0080),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::RightShift1, F2Function::Code9, false, false, 0), 0),
    ]);
    assert_eq!(out.l.raw() as u16, 0x8040,
        "MAGIC RSH: bit 15 = T's LSB = 1; (0x0080>>1) | 0x8000 = 0x8040");
}

#[test]
fn magic_lsh_inactive_without_f2_code9() {
    // No F2=MAGIC → plain LSH, no T injection regardless of T value.
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x8000),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x0042),
        // Plain LSH, no F2=Code9
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::LeftShift1, F2Function::Nop, false, false, 0), 0),
    ]);
    assert_eq!(out.l.raw() as u16, 0x0084,
        "Plain LSH (no MAGIC) produces L<<1 with no T injection");
}

#[test]
fn magic_inactive_in_disk_task() {
    // F2=Code9 is per-task; in disk task, F2=9 is RWC (NEXT-modify, not
    // MAGIC).  Plain LSH should produce L<<1 with no T injection.
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, true, false, 0), 0x8000)
            .with_task(4),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Constant, F2Function::Nop, false, true, 0), 0x0042)
            .with_task(4),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::LeftShift1, F2Function::Code9, false, false, 0), 0)
            .with_task(4),
    ]);
    assert_eq!(out.l.raw() as u16, 0x0084,
        "MAGIC is Emulator-only; in disk task, F2=Code9 is RWC, no T injection");
}

#[test]
fn acsource_inactive_in_disk_task() {
    // F2=Code14 (=ACSOURCE in Emulator) is per-task; in disk task
    // (F2=14 = STROBON per spec §8.5), ACSOURCE late dispatch must NOT
    // fire.  Stage IR via Emulator, switch to disk task, run F2=Code14.
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::MemoryData,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0x8000).with_task(0),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::Code14, false, false, 0x100), 0).with_task(4),
    ]);
    assert_eq!(out.next_mpc.raw() as u16, 0x100,
        "ACSOURCE late dispatch is Emulator-only; disk task must not modify NEXT");
}

#[test]
fn idisp_priority_ir0_wins_over_ir12() {
    // IR[0]=1 takes precedence over IR[1-2] checks per spec table.
    // IR=0xE000 has IR[0]=1 AND IR[1-2]=3.  Should use IR[0]=1 branch.
    // IR[8-9] = 0 → 3-0 = 3.  So next = 0x103 (NOT 0x103 from IR[4-7]
    // path which would also give 0).
    assert_eq!(idisp_dispatch_for_ir(0xE000), 0x103,
        "IR[0]=1 must take precedence over the IR[1-2]/IR[4-7] table");
}
