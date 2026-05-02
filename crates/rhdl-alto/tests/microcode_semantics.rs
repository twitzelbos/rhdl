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
fn bs_instruction_register_reads_ir() {
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0xDEAD).with_task(0),
        InCfg::new(ui(0, AluFunction::Bus, BusSource::InstructionRegister,
            F1Function::Nop, F2Function::Nop, true, false, 0), 0).with_task(0),
    ]);
    assert_eq!(out.t.raw() as u16, 0xDEAD);
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
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
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
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
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
    let out = observe_comb(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
            F1Function::Nop, F2Function::LoadIr, false, false, 0), 0)
            .with_md(0xCAFE).with_task(0),
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
    let out = observe_after(vec![
        // Cycle 0: load IR.
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
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
    let out = observe_after(vec![
        InCfg::new(ui(0, AluFunction::Bus, BusSource::ReadR,
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
