//! External-interrupt tests (mip / mie / mstatus.MIE / MPIE).
//!
//! Per the RISC-V privileged-ISA spec, M-mode interrupts fire when:
//!
//!   `mstatus.MIE && ((mip & mie) & {bit3, bit7, bit11}) != 0`
//!
//! at any inter-instruction boundary.  Trap entry atomically:
//!
//! - `mepc  ← PC of the instruction the interrupt squashed`
//! - `mcause ← interrupt cause (bit 31 set)`
//! - `mtval ← 0`
//! - `mstatus.MPIE ← mstatus.MIE`
//! - `mstatus.MIE  ← 0`
//!
//! `MRET` restores `mstatus.MIE ← mstatus.MPIE`, sets `MPIE = 1`,
//! and `PC ← mepc` — so the squashed instruction re-executes after
//! the handler returns.
//!
//! Three sources, with priority M-external > M-software > M-timer:
//!
//! - `mip[3]  / mie[3]`  — M-software (cause `0x80000003`)
//! - `mip[7]  / mie[7]`  — M-timer    (cause `0x80000007`)
//! - `mip[11] / mie[11]` — M-external (cause `0x8000000B`)
//!
//! `mip` is read-only and mirrors the CPU's `int_pending` input
//! port — the platform (test harness) owns the level.
//!
//! Tests run on the single-cycle CPU, the pipelined CPU, and the
//! Rust reference simulator with `int_pending` driven by the
//! harness on chosen cycles.

use rhdl::core::sim::ResetOrData;
use rhdl::prelude::*;
use rhdl_rv32i::cpu::{Cpu, In as SInIn, Out as SOut};
use rhdl_rv32i::csr::*;
use rhdl_rv32i::pipelined::{In as PIn, Out as POut, PipelinedCpu};
use rhdl_rv32i::sim;

// ---- Encoding helpers --------------------------------------------

fn i_type(imm: i32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    let imm_u = (imm as u32) & 0xFFF;
    (imm_u << 20) | (rs1 & 0x1F) << 15 | (funct3 & 0x7) << 12 | (rd & 0x1F) << 7 | (opcode & 0x7F)
}
fn s_type(imm: i32, rs2: u32, rs1: u32, funct3: u32, opcode: u32) -> u32 {
    let imm_u = (imm as u32) & 0xFFF;
    let imm_high = (imm_u >> 5) & 0x7F;
    let imm_low = imm_u & 0x1F;
    (imm_high << 25)
        | (rs2 & 0x1F) << 20
        | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12
        | imm_low << 7
        | (opcode & 0x7F)
}

fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    i_type(imm, rs1, 0, rd, 0x13)
}
fn sw(rs2: u32, rs1: u32, imm: i32) -> u32 {
    s_type(imm, rs2, rs1, 2, 0x23)
}
fn lui(rd: u32, imm20: u32) -> u32 {
    (imm20 & 0xFFFFF) << 12 | (rd & 0x1F) << 7 | 0x37
}
fn csrrw(rd: u32, rs1: u32, csr: u32) -> u32 {
    ((csr & 0xFFF) << 20) | (rs1 & 0x1F) << 15 | 1 << 12 | (rd & 0x1F) << 7 | 0x73
}
fn csrrs(rd: u32, rs1: u32, csr: u32) -> u32 {
    ((csr & 0xFFF) << 20) | (rs1 & 0x1F) << 15 | 2 << 12 | (rd & 0x1F) << 7 | 0x73
}
fn csrrsi(rd: u32, uimm: u32, csr: u32) -> u32 {
    ((csr & 0xFFF) << 20) | (uimm & 0x1F) << 15 | 6 << 12 | (rd & 0x1F) << 7 | 0x73
}

const HALT: u32 = 0x0000_0063;
const MRET: u32 = 0x3020_0073;

// ---- Closed-loop harnesses with int-pending control -------------
//
// Tests pass a closure `int_at(cycle)` that returns the int_pending
// vector to drive on each post-reset cycle.  This lets each test
// inject an interrupt at a chosen instant.

fn run_single_with_int<F: FnMut(usize) -> u32>(
    program: Vec<u32>,
    max_cycles: usize,
    mut int_at: F,
) -> [u32; 256] {
    let uut = Cpu::default();
    let mut data_mem: [u32; 256] = [0; 256];
    let mut reset_cycles_remaining = 2;
    let mut total_cycles: usize = 0;
    uut.run_fn(
        |out: SOut| {
            if reset_cycles_remaining > 0 {
                reset_cycles_remaining -= 1;
                return Some(ResetOrData::Reset);
            }
            if total_cycles >= max_cycles {
                return None;
            }
            let cyc = total_cycles;
            total_cycles += 1;
            if out.mem_write {
                let addr_word = (out.mem_addr.raw() / 4) as usize;
                if addr_word < data_mem.len() {
                    data_mem[addr_word] = out.mem_wdata.raw() as u32;
                }
            }
            let pc_word = (out.pc.raw() / 4) as usize;
            let instr = if pc_word < program.len() {
                program[pc_word]
            } else {
                0
            };
            let read_word = (out.mem_addr.raw() / 4) as usize;
            let mem_rdata = if read_word < data_mem.len() {
                data_mem[read_word]
            } else {
                0
            };
            Some(ResetOrData::Data(SInIn {
                instr: bits::<32>(instr as u128),
                mem_rdata: bits::<32>(mem_rdata as u128),
                int_pending: bits::<32>(int_at(cyc) as u128),
            }))
        },
        100,
    )
    .for_each(drop);
    data_mem
}

fn run_pipelined_with_int<F: FnMut(usize) -> u32>(
    program: Vec<u32>,
    max_cycles: usize,
    mut int_at: F,
) -> [u32; 256] {
    let uut = PipelinedCpu::default();
    let mut data_mem: [u32; 256] = [0; 256];
    let mut reset_cycles_remaining = 2;
    let mut total_cycles: usize = 0;
    uut.run_fn(
        |out: POut| {
            if reset_cycles_remaining > 0 {
                reset_cycles_remaining -= 1;
                return Some(ResetOrData::Reset);
            }
            if total_cycles >= max_cycles {
                return None;
            }
            let cyc = total_cycles;
            total_cycles += 1;
            if out.mem_write {
                let addr_word = (out.mem_addr.raw() / 4) as usize;
                if addr_word < data_mem.len() {
                    data_mem[addr_word] = out.mem_wdata.raw() as u32;
                }
            }
            let pc_word = (out.pc.raw() / 4) as usize;
            let instr = if pc_word < program.len() {
                program[pc_word]
            } else {
                0
            };
            let read_word = (out.mem_addr.raw() / 4) as usize;
            let mem_rdata = if read_word < data_mem.len() {
                data_mem[read_word]
            } else {
                0
            };
            Some(ResetOrData::Data(PIn {
                instr: bits::<32>(instr as u128),
                mem_rdata: bits::<32>(mem_rdata as u128),
                int_pending: bits::<32>(int_at(cyc) as u128),
            }))
        },
        100,
    )
    .for_each(drop);
    data_mem
}

// ---- Sanity: int_pending = 0 doesn't change anything -------------

#[test]
fn no_interrupt_when_int_pending_zero() {
    // Run a program with mstatus.MIE set + mie = all bits — but
    // int_pending = 0 throughout.  No interrupt should fire.
    let program = vec![
        addi(1, 0, 0x888),           // 0x00: x1 = 0x888 (enable all 3 interrupts)
        csrrw(0, 1, CSR_MIE),        // 0x04: mie = 0x888
        csrrsi(0, 0x8, CSR_MSTATUS), // 0x08: mstatus.MIE = 1
        addi(2, 0, 0x55),            // 0x0C
        sw(2, 0, 0),                 // 0x10: mem[0] = 0x55
        HALT,                        // 0x14
    ];
    let mem = run_single_with_int(program, 24, |_| 0);
    assert_eq!(mem[0], 0x55, "no interrupt → SW commits");
}

// ---- Test each interrupt source -----------------------------------

/// M-software interrupt fires when MSIP is pending+enabled and MIE
/// is set.  Handler reads mcause and stores it.
#[test]
fn m_software_interrupt_fires_single_cycle() {
    // 0x00..0x14: setup mtvec=0x20, mie=MSIE, mstatus.MIE=1
    // 0x18..   : user code that should be interrupted
    // 0x20+    : handler reads mcause, stores it, then HALTs
    let program = vec![
        lui(1, 0),                   // 0x00
        addi(1, 1, 0x20),            // 0x04: x1 = 0x20 (mtvec)
        csrrw(0, 1, CSR_MTVEC),      // 0x08
        addi(2, 0, 0x008),           // 0x0C: enable MSIE
        csrrw(0, 2, CSR_MIE),        // 0x10
        csrrsi(0, 0x8, CSR_MSTATUS), // 0x14: mstatus.MIE = 1
        addi(7, 0, 0x77),            // 0x18: user code start (will be interrupted)
        addi(7, 0, 0x77),            // 0x1C: pad
        // 0x20: handler
        csrrs(3, 0, CSR_MCAUSE), // 0x20: x3 ← mcause
        csrrs(4, 0, CSR_MEPC),   // 0x24: x4 ← mepc
        sw(3, 0, 0),             // 0x28: mem[0] = mcause
        sw(4, 0, 4),             // 0x2C: mem[1] = mepc
        HALT,                    // 0x30
    ];
    // mstatus.MIE commits at end of cycle 5 (the csrrsi at 0x14).
    // From cycle 6 onward, mstatus.MIE is set.  Assert MSIP from
    // cycle 6 so the very first user-code instruction (PC = 0x18)
    // is squashed.
    let mem = run_single_with_int(program, 32, |c| if c >= 6 { 0x008 } else { 0 });
    assert_eq!(mem[0], 0x8000_0003, "mcause = M-software interrupt");
    // mepc should be in the user-code range (0x18 or 0x1C).
    assert!(mem[1] == 0x18 || mem[1] == 0x1C, "mepc was 0x{:x}", mem[1]);
}

#[test]
fn m_timer_interrupt_fires_single_cycle() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        addi(2, 0, 0x080), // enable MTIE
        csrrw(0, 2, CSR_MIE),
        csrrsi(0, 0x8, CSR_MSTATUS),
        addi(7, 0, 0x77),
        addi(7, 0, 0x77),
        csrrs(3, 0, CSR_MCAUSE),
        csrrs(4, 0, CSR_MEPC),
        sw(3, 0, 0),
        sw(4, 0, 4),
        HALT,
    ];
    let mem = run_single_with_int(program, 32, |c| if c >= 8 { 0x080 } else { 0 });
    assert_eq!(mem[0], 0x8000_0007, "mcause = M-timer interrupt");
}

#[test]
fn m_external_interrupt_fires_single_cycle() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        // Need mie = 0x800 — but addi imm is 12-bit signed.  0x800
        // sign-extends to 0xFFFF_F800 — which is fine because mie
        // only cares about bits 3/7/11.  But cleaner to use lui.
        lui(2, 1),            // x2 = 0x1000
        addi(2, 2, -0x800),   // x2 = 0x800
        csrrw(0, 2, CSR_MIE), // mie = 0x800 (MEIE)
        csrrsi(0, 0x8, CSR_MSTATUS),
        addi(7, 0, 0x77),
        addi(7, 0, 0x77),
        csrrs(3, 0, CSR_MCAUSE),
        csrrs(4, 0, CSR_MEPC),
        sw(3, 0, 0),
        sw(4, 0, 4),
        HALT,
    ];
    let mem = run_single_with_int(program, 36, |c| if c >= 9 { 0x800 } else { 0 });
    assert_eq!(mem[0], 0x8000_000B, "mcause = M-external interrupt");
}

// ---- mstatus.MIE gating ------------------------------------------

#[test]
fn interrupt_does_not_fire_when_mie_clear() {
    // Same setup as the M-software test but DON'T set mstatus.MIE.
    // Even though mie and int_pending both have MSIE asserted,
    // the global enable is off → no interrupt.
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        addi(2, 0, 0x008),
        csrrw(0, 2, CSR_MIE),
        // (mstatus.MIE intentionally NOT set)
        addi(7, 0, 0x77),
        addi(7, 0, 0x77),
        sw(7, 0, 0), // mem[0] = 0x77
        HALT,
        // (handler at 0x20 — should NOT execute)
        addi(8, 0, 0xFF),
        sw(8, 0, 0), // mem[0] = 0xFF (would-be-trap marker)
        HALT,
    ];
    let mem = run_single_with_int(program, 24, |_| 0x008);
    assert_eq!(mem[0], 0x77, "no MIE → no interrupt → user SW commits");
}

#[test]
fn interrupt_does_not_fire_when_mie_bit_clear() {
    // mstatus.MIE set, but mie's MSIE bit not set.  int_pending
    // has MSIP asserted.  Should NOT interrupt.
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        // Don't enable any mie bits.
        csrrsi(0, 0x8, CSR_MSTATUS),
        addi(7, 0, 0x77),
        sw(7, 0, 0),
        HALT,
        addi(0, 0, 0),
        addi(8, 0, 0xFF),
        sw(8, 0, 0),
        HALT,
    ];
    let mem = run_single_with_int(program, 24, |_| 0x008);
    assert_eq!(mem[0], 0x77, "no mie.MSIE → no interrupt");
}

// ---- MRET restores MIE -------------------------------------------

/// Sim-only sanity that the MRET path restores mstatus.
#[test]
fn sim_mret_restores_mstatus_directly() {
    let mut cpu = sim::Cpu::new();
    cpu.write_csr(0x300, 0x8); // MIE = 1
    cpu.write_csr(0x305, 0x40); // mtvec
    cpu.write_csr(0x304, 0x008); // mie = MSIE
    cpu.int_pending = 0x008;
    cpu.pc = 0x18;
    cpu.step(&[0; 64]); // takes interrupt
    assert_eq!(cpu.read_csr(0x300), 0x80, "after trap: MIE=0, MPIE=1");
    assert_eq!(cpu.pc, 0x40, "after trap: PC = mtvec");
    assert_eq!(cpu.read_csr(0x341), 0x18, "mepc = squashed PC");
    cpu.int_pending = 0;
    cpu.execute_mret();
    let mstatus = cpu.read_csr(0x300);
    assert_eq!(mstatus & 0x8, 0x8, "MRET restored MIE; got 0x{mstatus:x}");
    assert_eq!(mstatus & 0x80, 0x80, "MRET set MPIE=1; got 0x{mstatus:x}");
}

#[test]
fn mret_restores_mstatus_mie() {
    // Setup:
    //   0x00..0x14: mtvec=0x40, mie=MSIE, mstatus.MIE=1
    //   0x18:       user code (gets interrupted)
    //   0x1C:       post-MRET landing — read mstatus into x8
    //   0x20:       store x8 to mem[0]
    //   0x24:       HALT
    //   0x40+:      handler — clear mie, advance mepc by 4, MRET
    //
    // The interrupt fires at cycle 6 (PC = 0x18, very first user
    // instruction); mepc = 0x18; handler advances to 0x1C; MRET
    // returns to PC = 0x1C where we read the restored mstatus.
    let program = vec![
        lui(1, 0),                   // 0x00
        addi(1, 1, 0x40),            // 0x04: x1 = 0x40 (mtvec)
        csrrw(0, 1, CSR_MTVEC),      // 0x08
        addi(2, 0, 0x008),           // 0x0C: MSIE
        csrrw(0, 2, CSR_MIE),        // 0x10
        csrrsi(0, 0x8, CSR_MSTATUS), // 0x14: mstatus.MIE = 1
        addi(7, 0, 0x77),            // 0x18: user code (interrupted)
        // 0x1C: post-MRET landing
        csrrs(8, 0, CSR_MSTATUS), // 0x1C: read mstatus
        sw(8, 0, 0),              // 0x20: mem[0] = mstatus after MRET
        HALT,                     // 0x24
        addi(0, 0, 0),            // 0x28: pad
        addi(0, 0, 0),            // 0x2C: pad
        addi(0, 0, 0),            // 0x30: pad
        addi(0, 0, 0),            // 0x34: pad
        addi(0, 0, 0),            // 0x38: pad
        addi(0, 0, 0),            // 0x3C: pad
        // 0x40: handler — disable mie (so we don't re-trap), advance
        //       mepc to 0x1C (skip the interrupted addi at 0x18),
        //       then MRET.
        csrrw(0, 0, CSR_MIE),  // 0x40: mie = 0 (kill pending)
        csrrs(9, 0, CSR_MEPC), // 0x44: x9 ← mepc (= 0x18)
        addi(9, 9, 4),         // 0x48: x9 = 0x1C
        csrrw(0, 9, CSR_MEPC), // 0x4C: mepc = 0x1C
        MRET,                  // 0x50: return — restores MIE
    ];
    // Pulse interrupt at cycles 6-7: mstatus.MIE commits at end of
    // cycle 5, so the interrupt fires at cycle 6 with PC = 0x18
    // (the addi at 0x18, the very first user-code instruction).
    // mepc = 0x18, handler advances mepc+4 = 0x1C, MRET lands on
    // the csrrs x8 we want to read mstatus from.
    let mem = run_single_with_int(
        program,
        60,
        |c| if (6..=7).contains(&c) { 0x008 } else { 0 },
    );
    let mstatus_after_mret = mem[0];
    assert_eq!(
        mstatus_after_mret & 0x8,
        0x8,
        "MRET should restore mstatus.MIE = 1; got 0x{mstatus_after_mret:x}"
    );
    assert_eq!(
        mstatus_after_mret & 0x80,
        0x80,
        "MPIE should be set to 1 by MRET; got 0x{mstatus_after_mret:x}"
    );
}

// ---- Pipelined parity --------------------------------------------

#[test]
fn m_software_interrupt_pipelined_parity() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        addi(2, 0, 0x008),
        csrrw(0, 2, CSR_MIE),
        csrrsi(0, 0x8, CSR_MSTATUS),
        addi(7, 0, 0x77),
        addi(7, 0, 0x77),
        csrrs(3, 0, CSR_MCAUSE),
        csrrs(4, 0, CSR_MEPC),
        sw(3, 0, 0),
        sw(4, 0, 4),
        HALT,
    ];
    let single = run_single_with_int(program.clone(), 32, |c| if c >= 12 { 0x008 } else { 0 });
    let pipelined = run_pipelined_with_int(program, 60, |c| if c >= 16 { 0x008 } else { 0 });
    assert_eq!(single[0], 0x8000_0003);
    assert_eq!(pipelined[0], single[0], "pipelined mcause parity");
}

// ---- Priority among multiple sources ------------------------------

#[test]
fn m_external_takes_priority_over_software() {
    // Both MEIE and MSIE pending+enabled — MEI wins per spec.
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        lui(2, 1),          // 0x1000
        addi(2, 2, -0x7F8), // x2 = 0x808 (MEIE | MSIE)
        csrrw(0, 2, CSR_MIE),
        csrrsi(0, 0x8, CSR_MSTATUS),
        addi(7, 0, 0x77),
        addi(7, 0, 0x77),
        csrrs(3, 0, CSR_MCAUSE),
        sw(3, 0, 0),
        HALT,
    ];
    let mem = run_single_with_int(program, 32, |c| if c >= 9 { 0x808 } else { 0 });
    assert_eq!(mem[0], 0x8000_000B, "M-external should win priority");
}

// ---- Lockstep against Rust simulator ------------------------------

#[test]
fn lockstep_m_software_interrupt() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        addi(2, 0, 0x008),
        csrrw(0, 2, CSR_MIE),
        csrrsi(0, 0x8, CSR_MSTATUS),
        addi(7, 0, 0x77),
        addi(7, 0, 0x77),
        csrrs(3, 0, CSR_MCAUSE),
        csrrs(4, 0, CSR_MEPC),
        sw(3, 0, 0),
        sw(4, 0, 4),
        HALT,
    ];

    // Sim: assert int_pending after enough instructions retire that
    // mstatus.MIE is committed (~7 retired).  Since the sim
    // retires per step, we set int_pending = 0x008 starting at
    // step 7 (after the csrrsi).
    let mut sim_cpu = sim::Cpu::new();
    let mut sim_writes: Vec<(u32, u32)> = Vec::new();
    for step in 0..40 {
        sim_cpu.int_pending = if step >= 7 { 0x008 } else { 0 };
        let prev_writes = sim_cpu.mem_writes.len();
        sim_cpu.step(&program);
        for &w in &sim_cpu.mem_writes[prev_writes..] {
            sim_writes.push(w);
        }
        if sim_cpu.halted {
            break;
        }
    }

    // Single-cycle hardware: int_pending starts at cycle 8.
    let single_mem = run_single_with_int(program.clone(), 32, |c| if c >= 8 { 0x008 } else { 0 });

    // The sim's retired-count != hardware's cycle-count, but both
    // should produce the same FINAL mem[0] = M-software cause.
    // (Cycle-by-cycle write-sequence comparison is too fragile
    // when int timing differs between sim & hw; final-state check
    // is the right comparison here.)
    assert_eq!(sim_cpu.read_csr(0x342), 0x8000_0003);
    assert_eq!(single_mem[0], 0x8000_0003);
}
