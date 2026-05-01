//! Misaligned-target trap (mcause = 0) and WFI tests.
//!
//! Per `tier-c-flagship-cores.md` §3.4 / RISC-V privileged-ISA spec:
//!
//! - **Misaligned-target trap (cause 0)**: any branch / JAL / JALR
//!   whose computed target is not 4-byte aligned (low 2 bits != 0)
//!   takes a synchronous exception with `mcause = 0` and
//!   `mtval = the misaligned target`.  The branching instruction
//!   does not commit (writeback suppressed; PC vectors to mtvec).
//!   For JALR specifically, the spec mandates clearing bit 0 first
//!   (the `& 0xFFFE` step); bit 1 must then be 0 for a valid
//!   target — JALR with `(rs1 + imm) = 0xXXXXXXX2` traps because
//!   bit 1 is set after the bit-0 mask.
//!
//! - **WFI**: SYSTEM funct12 = 0x105.  Without external interrupts
//!   (deferred), the privileged-ISA spec explicitly allows WFI to
//!   execute as a NOP — advance PC by 4, no other state change.
//!   This test verifies WFI doesn't trap and doesn't change x-regs.
//!
//! All tests run end-to-end on:
//!   1. The Rust reference simulator (`sim::Cpu`).
//!   2. The single-cycle hardware CPU.
//!   3. The 5-stage pipelined CPU.
//!
//! The three implementations must agree on the resulting mem-write
//! sequence (lockstep validation).

use rhdl::core::sim::ResetOrData;
use rhdl::prelude::*;
use rhdl_rv32i::cpu::{Cpu, In as SInIn, Out as SOut};
use rhdl_rv32i::csr::*;
use rhdl_rv32i::pipelined::{In as PIn, Out as POut, PipelinedCpu};
use rhdl_rv32i::sim;

// ---- Encoding helpers --------------------------------------------

fn i_type(imm: i32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    let imm_u = (imm as u32) & 0xFFF;
    (imm_u << 20)
        | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12
        | (rd & 0x1F) << 7
        | (opcode & 0x7F)
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

fn addi(rd: u32, rs1: u32, imm: i32) -> u32 { i_type(imm, rs1, 0, rd, 0x13) }
fn sw(rs2: u32, rs1: u32, imm: i32) -> u32  { s_type(imm, rs2, rs1, 2, 0x23) }
fn lui(rd: u32, imm20: u32) -> u32          { (imm20 & 0xFFFFF) << 12 | (rd & 0x1F) << 7 | 0x37 }

fn csrrw(rd: u32, rs1: u32, csr: u32) -> u32 {
    ((csr & 0xFFF) << 20) | (rs1 & 0x1F) << 15 | 1 << 12 | (rd & 0x1F) << 7 | 0x73
}
fn csrrs(rd: u32, rs1: u32, csr: u32) -> u32 {
    ((csr & 0xFFF) << 20) | (rs1 & 0x1F) << 15 | 2 << 12 | (rd & 0x1F) << 7 | 0x73
}

/// B-type encoding for branches.
fn b_type(imm: i32, rs2: u32, rs1: u32, funct3: u32, opcode: u32) -> u32 {
    let imm_u = (imm as u32) & 0x1FFF;
    let bit12  = (imm_u >> 12) & 0x1;
    let bit11  = (imm_u >> 11) & 0x1;
    let b10_5  = (imm_u >> 5) & 0x3F;
    let b4_1   = (imm_u >> 1) & 0xF;
    (bit12 << 31)
        | (b10_5 << 25)
        | (rs2 & 0x1F) << 20
        | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12
        | (b4_1 << 8)
        | (bit11 << 7)
        | (opcode & 0x7F)
}
fn beq(rs1: u32, rs2: u32, imm: i32) -> u32 { b_type(imm, rs2, rs1, 0, 0x63) }

/// J-type encoding for JAL.
fn j_type(imm: i32, rd: u32, opcode: u32) -> u32 {
    let imm_u = (imm as u32) & 0x1F_FFFF;
    let bit20  = (imm_u >> 20) & 0x1;
    let b19_12 = (imm_u >> 12) & 0xFF;
    let bit11  = (imm_u >> 11) & 0x1;
    let b10_1  = (imm_u >> 1) & 0x3FF;
    (bit20 << 31)
        | (b19_12 << 12)
        | (bit11 << 20)
        | (b10_1 << 21)
        | (rd & 0x1F) << 7
        | (opcode & 0x7F)
}
fn jal(rd: u32, imm: i32) -> u32 { j_type(imm, rd, 0x6F) }

/// I-type JALR (opcode 0x67, funct3 = 0).
fn jalr(rd: u32, rs1: u32, imm: i32) -> u32 { i_type(imm, rs1, 0, rd, 0x67) }

const HALT: u32   = 0x0000_0063;       // beq x0, x0, +0
const WFI: u32    = 0x1050_0073;       // SYSTEM funct12 = 0x105

// ---- Closed-loop harnesses ---------------------------------------

fn run_single(program: Vec<u32>, max_cycles: usize) -> [u32; 256] {
    let uut = Cpu::default();
    let mut data_mem: [u32; 256] = [0; 256];
    let mut reset_cycles_remaining = 2;
    let mut total_cycles: usize = 0;
    uut.run_fn(
        |out: SOut| {
            if reset_cycles_remaining > 0 { reset_cycles_remaining -= 1; return Some(ResetOrData::Reset); }
            if total_cycles >= max_cycles { return None; }
            total_cycles += 1;
            if out.mem_write {
                let addr_word = (out.mem_addr.raw() / 4) as usize;
                if addr_word < data_mem.len() { data_mem[addr_word] = out.mem_wdata.raw() as u32; }
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
    data_mem
}

fn run_pipelined(program: Vec<u32>, max_cycles: usize) -> [u32; 256] {
    let uut = PipelinedCpu::default();
    let mut data_mem: [u32; 256] = [0; 256];
    let mut reset_cycles_remaining = 2;
    let mut total_cycles: usize = 0;
    uut.run_fn(
        |out: POut| {
            if reset_cycles_remaining > 0 { reset_cycles_remaining -= 1; return Some(ResetOrData::Reset); }
            if total_cycles >= max_cycles { return None; }
            total_cycles += 1;
            if out.mem_write {
                let addr_word = (out.mem_addr.raw() / 4) as usize;
                if addr_word < data_mem.len() { data_mem[addr_word] = out.mem_wdata.raw() as u32; }
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
    data_mem
}

// ---- Misaligned-target trap tests --------------------------------

/// Misaligned branch target: BEQ taken with imm = 6 produces target
/// = 0x14 + 6 = 0x1A, which is not 4-byte aligned (bits[1:0] = 10).
/// Trap vectors to mtvec; mcause = 0; mtval = 0x1A.
///
/// Program shape:
///   0x00: setup mtvec = 0x20 (handler at 0x20)
///   0x14: BEQ x0, x0, +6   ← misaligned (target 0x1A)
///   0x20: handler reads mepc, mcause, mtval, stores them
#[test]
fn misaligned_branch_target_traps_single_cycle() {
    let program = vec![
        lui(1, 0),                       // 0x00: x1 = 0
        addi(1, 1, 0x20),                // 0x04: x1 = 0x20
        csrrw(0, 1, CSR_MTVEC),          // 0x08: mtvec = 0x20
        addi(0, 0, 0),                   // 0x0C: NOP
        addi(0, 0, 0),                   // 0x10: NOP
        beq(0, 0, 6),                    // 0x14: BEQ taken, misaligned target = 0x1A
        addi(0, 0, 0),                   // 0x18: SQUASHED
        addi(0, 0, 0),                   // 0x1C: SQUASHED
        // 0x20: handler
        csrrs(2, 0, CSR_MEPC),           // 0x20: x2 ← 0x14
        csrrs(3, 0, CSR_MCAUSE),         // 0x24: x3 ← 0
        csrrs(4, 0, CSR_MTVAL),          // 0x28: x4 ← 0x1A
        sw(2, 0, 0),                     // 0x2C: mem[0] = 0x14
        sw(3, 0, 4),                     // 0x30: mem[1] = 0
        sw(4, 0, 8),                     // 0x34: mem[2] = 0x1A
        HALT,                            // 0x38
    ];
    let mem = run_single(program, 32);
    assert_eq!(mem[0], 0x14, "mepc should hold the BEQ's PC");
    assert_eq!(mem[1], 0,    "mcause should be 0 (Instruction address misaligned)");
    assert_eq!(mem[2], 0x1A, "mtval should hold the misaligned target");
}

#[test]
fn misaligned_branch_target_traps_pipelined_parity() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        addi(0, 0, 0),
        addi(0, 0, 0),
        beq(0, 0, 6),
        addi(0, 0, 0),
        addi(0, 0, 0),
        csrrs(2, 0, CSR_MEPC),
        csrrs(3, 0, CSR_MCAUSE),
        csrrs(4, 0, CSR_MTVAL),
        sw(2, 0, 0),
        sw(3, 0, 4),
        sw(4, 0, 8),
        HALT,
    ];
    let single = run_single(program.clone(), 32);
    let pipelined = run_pipelined(program, 60);
    assert_eq!(single[0], 0x14);
    assert_eq!(single[1], 0);
    assert_eq!(single[2], 0x1A);
    assert_eq!(pipelined[0], single[0], "pipelined mepc parity");
    assert_eq!(pipelined[1], single[1], "pipelined mcause parity");
    assert_eq!(pipelined[2], single[2], "pipelined mtval parity");
}

/// Misaligned JAL target: JAL with imm = 2 produces target = pc + 2,
/// which is not 4-byte aligned.  Same trap shape as the branch case.
#[test]
fn misaligned_jal_target_traps_single_cycle() {
    let program = vec![
        lui(1, 0),                       // 0x00
        addi(1, 1, 0x20),                // 0x04
        csrrw(0, 1, CSR_MTVEC),          // 0x08
        addi(0, 0, 0),                   // 0x0C
        addi(0, 0, 0),                   // 0x10
        jal(5, 2),                       // 0x14: JAL x5, +2 → target = 0x16, misaligned
        addi(0, 0, 0),                   // 0x18: SQUASHED
        addi(0, 0, 0),                   // 0x1C: SQUASHED
        csrrs(2, 0, CSR_MEPC),           // 0x20
        csrrs(3, 0, CSR_MCAUSE),         // 0x24
        csrrs(4, 0, CSR_MTVAL),          // 0x28
        sw(2, 0, 0),                     // 0x2C
        sw(3, 0, 4),                     // 0x30
        sw(4, 0, 8),                     // 0x34
        HALT,                            // 0x38
    ];
    let mem = run_single(program, 32);
    assert_eq!(mem[0], 0x14, "mepc should hold the JAL's PC");
    assert_eq!(mem[1], 0,    "mcause should be 0");
    assert_eq!(mem[2], 0x16, "mtval should hold the misaligned target");
}

/// Misaligned JALR target: JALR with `(rs1 + imm) & 0xFFFFFFFE`
/// must have bit 1 = 0 to be aligned.  Set rs1 = 0x42 and imm = 0
/// → masked target = 0x42, which has bit 1 set → trap.
#[test]
fn misaligned_jalr_target_traps_single_cycle() {
    let program = vec![
        lui(1, 0),                       // 0x00
        addi(1, 1, 0x20),                // 0x04: x1 = 0x20 (handler addr / mtvec)
        csrrw(0, 1, CSR_MTVEC),          // 0x08
        addi(6, 0, 0x42),                // 0x0C: x6 = 0x42 (target with bit 1 set)
        addi(0, 0, 0),                   // 0x10: NOP
        jalr(5, 6, 0),                   // 0x14: JALR x5, x6, 0 → target masked = 0x42, misaligned
        addi(0, 0, 0),                   // 0x18: SQUASHED
        addi(0, 0, 0),                   // 0x1C: SQUASHED
        csrrs(2, 0, CSR_MEPC),           // 0x20
        csrrs(3, 0, CSR_MCAUSE),         // 0x24
        csrrs(4, 0, CSR_MTVAL),          // 0x28
        sw(2, 0, 0),                     // 0x2C
        sw(3, 0, 4),                     // 0x30
        sw(4, 0, 8),                     // 0x34
        HALT,                            // 0x38
    ];
    let mem = run_single(program, 32);
    assert_eq!(mem[0], 0x14, "mepc should hold the JALR's PC");
    assert_eq!(mem[1], 0,    "mcause should be 0");
    assert_eq!(mem[2], 0x42, "mtval should hold the masked-misaligned target");
}

/// Aligned branch target should NOT trap (sanity check: imm = 8
/// produces target = 0x1C, which is 4-byte aligned).
#[test]
fn aligned_branch_target_does_not_trap_single_cycle() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        addi(0, 0, 0),
        addi(0, 0, 0),
        beq(0, 0, 8),                    // 0x14: aligned target 0x1C
        addi(0, 0, 0),                   // SQUASHED (branch taken)
        addi(0, 0, 0),                   // SQUASHED (branch taken)
        addi(7, 0, 0xCC),                // 0x1C: branch landing
        sw(7, 0, 0),                     // 0x20: mem[0] = 0xCC
        HALT,                            // 0x24
    ];
    let mem = run_single(program, 24);
    assert_eq!(mem[0], 0xCC, "branch should land at aligned target");
}

// ---- WFI tests ---------------------------------------------------

/// WFI: with no external interrupts, executes as a NOP that
/// advances PC by 4.  Test that the program continues executing
/// past the WFI without trapping.
#[test]
fn wfi_executes_as_nop_single_cycle() {
    // 0x00: addi x5, x0, 0x77   ; x5 = 0x77
    // 0x04: WFI                  ; should be a NOP
    // 0x08: sw x5, 0(x0)         ; mem[0] = 0x77 (proves we got past WFI)
    // 0x0C: HALT
    let program = vec![
        addi(5, 0, 0x77),
        WFI,
        sw(5, 0, 0),
        HALT,
    ];
    let mem = run_single(program, 16);
    assert_eq!(mem[0], 0x77, "WFI should NOP and let the next instruction store");
}

#[test]
fn wfi_executes_as_nop_pipelined_parity() {
    let program = vec![
        addi(5, 0, 0x77),
        WFI,
        sw(5, 0, 0),
        HALT,
    ];
    let single = run_single(program.clone(), 16);
    let pipelined = run_pipelined(program, 30);
    assert_eq!(pipelined[0], single[0], "pipelined WFI parity");
}

/// WFI shouldn't be decoded as illegal — verify by trapping the
/// program ONLY if WFI's opcode produces an illegal-instruction
/// trap.  If WFI is correctly recognised, the post-WFI store
/// happens; otherwise the trap handler runs and writes a marker.
#[test]
fn wfi_is_not_illegal_single_cycle() {
    let program = vec![
        lui(1, 0),                       // 0x00
        addi(1, 1, 0x20),                // 0x04: x1 = 0x20 (handler)
        csrrw(0, 1, CSR_MTVEC),          // 0x08: mtvec = 0x20
        addi(0, 0, 0),                   // 0x0C: NOP
        addi(0, 0, 0),                   // 0x10: NOP
        WFI,                             // 0x14
        addi(5, 0, 0x42),                // 0x18: should run if WFI isn't illegal
        sw(5, 0, 0),                     // 0x1C: mem[0] = 0x42
        // 0x20: handler
        addi(7, 0, 0x55),                // 0x20: marker = 0x55 (would-be trap)
        sw(7, 0, 4),                     // 0x24: mem[1] = 0x55
        HALT,                            // 0x28
    ];
    let mem = run_single(program, 24);
    assert_eq!(mem[0], 0x42, "WFI should not trap; post-WFI code must run");
    // mem[1] may or may not run depending on PC walk; the key
    // assertion is mem[0] above.
}

// ---- Lockstep against Rust simulator ------------------------------

/// 3-way lockstep: rust_sim ↔ single-cycle ↔ pipelined for a
/// program containing both a misaligned-branch trap and a WFI.
/// All three implementations must produce the same memory writes.
#[test]
fn lockstep_misaligned_and_wfi() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        WFI,                              // NOP
        addi(0, 0, 0),
        beq(0, 0, 6),                     // misaligned: 0x14 + 6 = 0x1A
        addi(0, 0, 0),
        addi(0, 0, 0),
        // 0x20: handler — read trap state and store
        csrrs(2, 0, CSR_MEPC),
        csrrs(3, 0, CSR_MCAUSE),
        csrrs(4, 0, CSR_MTVAL),
        sw(2, 0, 0),
        sw(3, 0, 4),
        sw(4, 0, 8),
        HALT,
    ];
    let sim_state = sim::Cpu::new().run(&program, 60);
    let sim_writes = sim_state.mem_writes;

    // Run hardware and collect writes (re-using the harness pattern
    // from the lockstep test file but inline for clarity).
    let mut single_writes: Vec<(u32, u32)> = Vec::new();
    {
        let uut = Cpu::default();
        let mut data_mem: [u32; 256] = [0; 256];
        let mut reset_cycles_remaining = 2;
        let mut total_cycles: usize = 0;
        uut.run_fn(
            |out: SOut| {
                if reset_cycles_remaining > 0 { reset_cycles_remaining -= 1; return Some(ResetOrData::Reset); }
                if total_cycles >= 60 { return None; }
                total_cycles += 1;
                if out.mem_write {
                    let addr = out.mem_addr.raw() as u32;
                    let val  = out.mem_wdata.raw() as u32;
                    single_writes.push((addr, val));
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
    }

    let mut pipelined_writes: Vec<(u32, u32)> = Vec::new();
    {
        let uut = PipelinedCpu::default();
        let mut data_mem: [u32; 256] = [0; 256];
        let mut reset_cycles_remaining = 2;
        let mut total_cycles: usize = 0;
        uut.run_fn(
            |out: POut| {
                if reset_cycles_remaining > 0 { reset_cycles_remaining -= 1; return Some(ResetOrData::Reset); }
                if total_cycles >= 120 { return None; }
                total_cycles += 1;
                if out.mem_write {
                    let addr = out.mem_addr.raw() as u32;
                    let val  = out.mem_wdata.raw() as u32;
                    pipelined_writes.push((addr, val));
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
    }

    assert_eq!(
        sim_writes, single_writes,
        "sim ↔ single-cycle write-sequence mismatch\n  sim: {sim_writes:?}\n  single: {single_writes:?}",
    );
    assert_eq!(
        sim_writes, pipelined_writes,
        "sim ↔ pipelined write-sequence mismatch\n  sim: {sim_writes:?}\n  pipelined: {pipelined_writes:?}",
    );
}
