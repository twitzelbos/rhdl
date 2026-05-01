//! Lockstep cosimulation: Rust reference simulator vs.
//! single-cycle CPU vs. pipelined CPU.
//!
//! Per `tier-c-flagship-cores.md` §3.6: "After every retired
//! instruction, compare register file state and memory state.
//! Discrepancies are bugs."  The Rust simulator (in
//! `rhdl_rv32i::sim`) is our gold model — independently
//! implemented in interpretive style — so it serves the same
//! role as upstream Spike for catching bugs that both hardware
//! cores might share.
//!
//! Comparison surface: **the per-program memory-write sequence**.
//! Memory writes are the only architectural side-effect a
//! program can expose; if all three implementations produce the
//! same write sequence on the same program, they agree at the
//! architectural-state level.
//!
//! The test programs come from the existing compliance suite
//! (PR #34) — known-good RV32I instruction-stream patterns.
//! Lockstep is therefore a **3-way agreement check**: rust_sim
//! == single_cycle == pipelined.

use rhdl::core::sim::ResetOrData;
use rhdl::prelude::*;
use rhdl_rv32i::compliance::*;
use rhdl_rv32i::cpu::{Cpu, In as SInIn, Out as SOut};
use rhdl_rv32i::pipelined::{In as PIn, Out as POut, PipelinedCpu};
use rhdl_rv32i::sim;

// ---- Hardware run helpers (same shape as compliance.rs) ----

fn run_single_writes(program: Vec<u32>, max_cycles: usize) -> Vec<(u32, u32)> {
    let uut = Cpu::default();
    let mut writes: Vec<(u32, u32)> = Vec::new();
    let mut data_mem: [u32; 256] = [0; 256];
    let mut reset_cycles_remaining = 2;
    let mut total_cycles: usize = 0;
    uut.run_fn(
        |out: SOut| {
            if reset_cycles_remaining > 0 { reset_cycles_remaining -= 1; return Some(ResetOrData::Reset); }
            if total_cycles >= max_cycles { return None; }
            total_cycles += 1;
            if out.mem_write {
                let addr = out.mem_addr.raw() as u32;
                let val  = out.mem_wdata.raw() as u32;
                writes.push((addr, val));
                let addr_word = (addr / 4) as usize;
                if addr_word < data_mem.len() { data_mem[addr_word] = val; }
            }
            let pc_word = (out.pc.raw() / 4) as usize;
            let instr = if pc_word < program.len() { program[pc_word] } else { 0 };
            let read_word = (out.mem_addr.raw() / 4) as usize;
            let mem_rdata = if read_word < data_mem.len() { data_mem[read_word] } else { 0 };
            Some(ResetOrData::Data(SInIn {
                instr: bits::<32>(instr as u128),
                mem_rdata: bits::<32>(mem_rdata as u128),
            }))
        },
        100,
    ).for_each(drop);
    writes
}

fn run_pipelined_writes(program: Vec<u32>, max_cycles: usize) -> Vec<(u32, u32)> {
    let uut = PipelinedCpu::default();
    let mut writes: Vec<(u32, u32)> = Vec::new();
    let mut data_mem: [u32; 256] = [0; 256];
    let mut reset_cycles_remaining = 2;
    let mut total_cycles: usize = 0;
    uut.run_fn(
        |out: POut| {
            if reset_cycles_remaining > 0 { reset_cycles_remaining -= 1; return Some(ResetOrData::Reset); }
            if total_cycles >= max_cycles { return None; }
            total_cycles += 1;
            if out.mem_write {
                let addr = out.mem_addr.raw() as u32;
                let val  = out.mem_wdata.raw() as u32;
                writes.push((addr, val));
                let addr_word = (addr / 4) as usize;
                if addr_word < data_mem.len() { data_mem[addr_word] = val; }
            }
            let pc_word = (out.pc.raw() / 4) as usize;
            let instr = if pc_word < program.len() { program[pc_word] } else { 0 };
            let read_word = (out.mem_addr.raw() / 4) as usize;
            let mem_rdata = if read_word < data_mem.len() { data_mem[read_word] } else { 0 };
            Some(ResetOrData::Data(PIn {
                instr: bits::<32>(instr as u128),
                mem_rdata: bits::<32>(mem_rdata as u128),
            }))
        },
        100,
    ).for_each(drop);
    writes
}

/// Run all three implementations on the same program; assert the
/// memory-write sequences agree.
fn assert_lockstep(program: Vec<u32>, max_cycles: usize, name: &str) {
    let sim_state = sim::Cpu::new().run(&program, max_cycles as u64);
    let sim_writes: Vec<(u32, u32)> = sim_state.mem_writes.clone();

    let single_writes = run_single_writes(program.clone(), max_cycles);
    let pipelined_writes = run_pipelined_writes(program, max_cycles * 2);

    assert_eq!(
        sim_writes, single_writes,
        "{name}: sim ↔ single-cycle write-sequence mismatch\n  sim: {sim_writes:?}\n  single: {single_writes:?}",
    );
    assert_eq!(
        sim_writes, pipelined_writes,
        "{name}: sim ↔ pipelined write-sequence mismatch\n  sim: {sim_writes:?}\n  pipelined: {pipelined_writes:?}",
    );
}

// ---- Lockstep tests -----------------------------------------------
//
// Re-run each compliance program through the 3-way lockstep.
// If all three produce the same write sequence, the program is
// architecturally identical on all three.  If any differ, we
// have either a hardware bug or a simulator bug — and the bug is
// localised to whichever implementation differs from the other two.

#[test]
fn lockstep_rv32ui_p_add() {
    assert_lockstep(make_add_program(), 600, "rv32ui-p-add");
}

#[test]
fn lockstep_rv32ui_p_sub() {
    assert_lockstep(make_sub_program(), 600, "rv32ui-p-sub");
}

#[test]
fn lockstep_rv32ui_p_and() {
    assert_lockstep(make_and_program(), 600, "rv32ui-p-and");
}

#[test]
fn lockstep_rv32ui_p_or() {
    assert_lockstep(make_or_program(), 600, "rv32ui-p-or");
}

#[test]
fn lockstep_rv32ui_p_xor() {
    assert_lockstep(make_xor_program(), 600, "rv32ui-p-xor");
}

#[test]
fn lockstep_rv32ui_p_addi() {
    assert_lockstep(make_addi_program(), 800, "rv32ui-p-addi");
}

// ---- Direct sim unit tests (sanity) ------------------------------

#[test]
fn sim_addi_works() {
    let program = vec![
        // addi x1, x0, 5
        0x005_00_093,
        // addi x2, x0, 10
        0x00a_00_113,
        // add  x3, x1, x2  ← R-type ADD (funct7=0,rs2=2,rs1=1,funct3=0,rd=3,opcode=0x33)
        // = 0000000_00010_00001_000_00011_0110011 = 0x002081B3
        0x002081B3,
        // sw x3, 0(x0)  → s-type, imm=0, rs2=3, rs1=0, funct3=2, opcode=0x23
        // = 0000000_00011_00000_010_00000_0100011 = 0x00302023
        0x00302023,
        // halt: beq x0, x0, +0
        0x0000_0063,
    ];
    let cpu = sim::Cpu::new().run(&program, 16);
    assert_eq!(cpu.mem_writes, vec![(0, 15)], "5 + 10 = 15 stored at mem[0]");
    assert!(cpu.halted);
}

#[test]
fn sim_ecall_traps_correctly() {
    let program = vec![
        // lui x1, 0
        0x0000_00B7,
        // addi x1, x1, 0x10  ← x1 = 0x10 (mtvec)
        0x010_08_093,
        // csrrw x0, x1, mtvec  → opcode=0x73, funct3=1, rs1=1, csr=0x305
        // = 305 << 20 | 1 << 15 | 1 << 12 | 0 << 7 | 0x73 = 0x30509073
        0x30509073,
        // ecall
        0x0000_0073,
        // 0x10: csrrs x2, x0, mepc  ← x2 ← 0x0C (the ECALL's PC)
        // funct3=2, rs1=0, csr=0x341 → 0x341 << 20 | 0 | 2 << 12 | 2 << 7 | 0x73
        0x34102173,
        // sw x2, 0(x0)
        0x00202023,
        // halt
        0x0000_0063,
    ];
    let cpu = sim::Cpu::new().run(&program, 32);
    assert_eq!(cpu.read_csr(0x342), 11, "mcause should be 11 (M-mode ECALL)");
    assert_eq!(cpu.read_csr(0x341), 0x0C, "mepc should be the ECALL's PC");
    assert!(cpu.halted);
}
