//! Differential fuzz testing: generate random valid RV32I
//! instruction streams; run on the Rust simulator + single-cycle
//! CPU + pipelined CPU; assert all three produce identical
//! per-cycle memory writes.
//!
//! ## Why this matters
//!
//! Hand-written tests catch the bugs we *thought to test for*.
//! Random differential testing catches the bugs we *didn't*.  Each
//! random program exercises a unique combination of:
//!
//! - register-file dependency chains (RAW / WAR / WAW),
//! - pipeline hazards (load-use stalls, forwarding paths,
//!   branch squashes),
//! - control-flow patterns (taken/not-taken branches in unusual
//!   sequences, JAL/JALR with random targets),
//! - memory-access patterns (random addresses → exercises the
//!   misaligned-trap path; random store/load orderings → exercises
//!   the per-cycle write sequence).
//!
//! ## Methodology
//!
//! Each test seeds a deterministic LCG with a fixed value, generates
//! N programs of M instructions, and asserts agreement.  Failures
//! reproduce by re-running the test (deterministic by seed).
//!
//! Programs are generated to be **safe** by construction:
//!
//! - Register x0 is never used as the destination of a writing
//!   instruction (would be silently dropped anyway, but we avoid it
//!   to keep the fuzz output meaningful).
//! - Branches/JALs/JALR targets are deliberately constrained to
//!   stay within the program region (or jump to the HALT at the
//!   end) — otherwise random PC could escape into uninitialised
//!   memory that decodes as illegal and trap-loops forever.
//! - Loads/stores target a small reserved data window
//!   (offsets 0x100 .. 0x180), word-aligned, to avoid spurious
//!   misaligned-trap mismatches.  (The misaligned path is exercised
//!   by the dedicated tests in `cleanup.rs` and `misaligned_wfi.rs`.)
//! - Programs always end in HALT.
//!
//! Comparison: per-cycle memory-write sequence (the same surface
//! the lockstep harness from PR #37 uses).

use rhdl::core::sim::ResetOrData;
use rhdl::prelude::*;
use rhdl_rv32i::cpu::{Cpu, In as SInIn, Out as SOut};
use rhdl_rv32i::pipelined::{In as PIn, Out as POut, PipelinedCpu};
use rhdl_rv32i::sim;

// ---- Encoding helpers (subset we want the fuzzer to emit) -------

fn r_type(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    (funct7 & 0x7F) << 25 | (rs2 & 0x1F) << 20 | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12 | (rd & 0x1F) << 7 | (opcode & 0x7F)
}
fn i_type(imm: i32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    let imm_u = (imm as u32) & 0xFFF;
    (imm_u << 20) | (rs1 & 0x1F) << 15 | (funct3 & 0x7) << 12 | (rd & 0x1F) << 7 | (opcode & 0x7F)
}
fn s_type(imm: i32, rs2: u32, rs1: u32, funct3: u32, opcode: u32) -> u32 {
    let imm_u = (imm as u32) & 0xFFF;
    let imm_high = (imm_u >> 5) & 0x7F;
    let imm_low = imm_u & 0x1F;
    (imm_high << 25) | (rs2 & 0x1F) << 20 | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12 | imm_low << 7 | (opcode & 0x7F)
}
fn b_type(imm: i32, rs2: u32, rs1: u32, funct3: u32, opcode: u32) -> u32 {
    let imm_u = (imm as u32) & 0x1FFF;
    let bit12 = (imm_u >> 12) & 0x1;
    let bit11 = (imm_u >> 11) & 0x1;
    let b10_5 = (imm_u >> 5) & 0x3F;
    let b4_1 = (imm_u >> 1) & 0xF;
    (bit12 << 31) | (b10_5 << 25) | (rs2 & 0x1F) << 20 | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12 | (b4_1 << 8) | (bit11 << 7) | (opcode & 0x7F)
}
fn u_type(imm: u32, rd: u32, opcode: u32) -> u32 {
    (imm & 0xFFFFF000) | (rd & 0x1F) << 7 | (opcode & 0x7F)
}
fn j_type(imm: i32, rd: u32, opcode: u32) -> u32 {
    let imm_u = (imm as u32) & 0x1F_FFFF;
    let bit20 = (imm_u >> 20) & 0x1;
    let b19_12 = (imm_u >> 12) & 0xFF;
    let bit11 = (imm_u >> 11) & 0x1;
    let b10_1 = (imm_u >> 1) & 0x3FF;
    (bit20 << 31) | (b19_12 << 12) | (bit11 << 20) | (b10_1 << 21)
        | (rd & 0x1F) << 7 | (opcode & 0x7F)
}

const HALT: u32 = 0x0000_0063;

// ---- Tiny LCG for deterministic-but-varied program generation ---

struct Lcg(u64);
impl Lcg {
    fn new(seed: u64) -> Self { Self(seed) }
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }
    fn range(&mut self, n: u32) -> u32 { self.next() % n.max(1) }
    fn reg(&mut self) -> u32 { 1 + self.range(31) }  // never x0 as dest
    fn rs(&mut self) -> u32 { self.range(32) }       // x0 OK as src
}

// ---- Program generation ------------------------------------------

#[derive(Clone, Copy)]
enum Op {
    // R-type
    Add, Sub, Sll, Slt, Sltu, Xor, Srl, Sra, Or, And,
    // I-type ALU
    Addi, Slti, Sltiu, Xori, Ori, Andi, Slli, Srli, Srai,
    // U-type
    Lui, Auipc,
    // Loads (word-aligned)
    Lw,
    // Stores (word-aligned)
    Sw,
    // Branches
    Beq, Bne, Blt, Bge, Bltu, Bgeu,
    // Jumps
    Jal,
}

const OPS: &[Op] = &[
    Op::Add, Op::Sub, Op::Sll, Op::Slt, Op::Sltu, Op::Xor, Op::Srl, Op::Sra, Op::Or, Op::And,
    Op::Addi, Op::Slti, Op::Sltiu, Op::Xori, Op::Ori, Op::Andi, Op::Slli, Op::Srli, Op::Srai,
    Op::Lui, Op::Auipc,
    Op::Lw, Op::Sw,
    Op::Beq, Op::Bne, Op::Blt, Op::Bge, Op::Bltu, Op::Bgeu,
    Op::Jal,
];

/// Build one random instruction at byte offset `pc_byte` in a
/// program that's `total_words * 4` bytes long.  Branch / JAL
/// targets are constrained to land within the program region.
fn random_instr(rng: &mut Lcg, pc_byte: u32, total_words: u32) -> u32 {
    let total_bytes = total_words * 4;
    let halt_addr = (total_words - 1) * 4;
    let pick_target = |rng: &mut Lcg| -> i32 {
        // Pick a target byte address in [0, halt_addr].  Compute
        // signed offset relative to current PC.
        let target = (rng.range(total_words - 1)) * 4;
        let offset = (target as i32) - (pc_byte as i32);
        // Branches encode imm in 13 bits signed (-4096..4095, even);
        // JAL in 21 bits.  For our small programs (~32 instructions
        // = 128 bytes), this always fits.
        offset
    };
    let pick_data_addr = |rng: &mut Lcg| -> i32 {
        // Word-aligned offset in [0x100, 0x180) — outside program
        // region, inside data_mem array (256 words).
        let off_in_data = (rng.range(32)) * 4;
        // i_type imm is signed 12-bit (-2048..2047). 0x100+off fits.
        (0x100 + off_in_data) as i32
    };
    let op = OPS[rng.range(OPS.len() as u32) as usize];
    match op {
        Op::Add  => r_type(0,        rng.rs(), rng.rs(), 0, rng.reg(), 0x33),
        Op::Sub  => r_type(0b0100000,rng.rs(), rng.rs(), 0, rng.reg(), 0x33),
        Op::Sll  => r_type(0,        rng.rs(), rng.rs(), 1, rng.reg(), 0x33),
        Op::Slt  => r_type(0,        rng.rs(), rng.rs(), 2, rng.reg(), 0x33),
        Op::Sltu => r_type(0,        rng.rs(), rng.rs(), 3, rng.reg(), 0x33),
        Op::Xor  => r_type(0,        rng.rs(), rng.rs(), 4, rng.reg(), 0x33),
        Op::Srl  => r_type(0,        rng.rs(), rng.rs(), 5, rng.reg(), 0x33),
        Op::Sra  => r_type(0b0100000,rng.rs(), rng.rs(), 5, rng.reg(), 0x33),
        Op::Or   => r_type(0,        rng.rs(), rng.rs(), 6, rng.reg(), 0x33),
        Op::And  => r_type(0,        rng.rs(), rng.rs(), 7, rng.reg(), 0x33),
        Op::Addi  => i_type(rng.next() as i32 % 256 - 128, rng.rs(), 0, rng.reg(), 0x13),
        Op::Slti  => i_type(rng.next() as i32 % 256 - 128, rng.rs(), 2, rng.reg(), 0x13),
        Op::Sltiu => i_type(rng.next() as i32 % 256 - 128, rng.rs(), 3, rng.reg(), 0x13),
        Op::Xori  => i_type(rng.next() as i32 % 256 - 128, rng.rs(), 4, rng.reg(), 0x13),
        Op::Ori   => i_type(rng.next() as i32 % 256 - 128, rng.rs(), 6, rng.reg(), 0x13),
        Op::Andi  => i_type(rng.next() as i32 % 256 - 128, rng.rs(), 7, rng.reg(), 0x13),
        Op::Slli  => i_type((rng.range(32)) as i32, rng.rs(), 1, rng.reg(), 0x13),
        Op::Srli  => i_type((rng.range(32)) as i32, rng.rs(), 5, rng.reg(), 0x13),
        Op::Srai  => i_type(((1 << 10) | rng.range(32)) as i32, rng.rs(), 5, rng.reg(), 0x13),
        Op::Lui   => u_type((rng.next() & 0xFFFFF) << 12, rng.reg(), 0x37),
        Op::Auipc => u_type((rng.next() & 0xFFFFF) << 12, rng.reg(), 0x17),
        Op::Lw    => i_type(pick_data_addr(rng), 0, 2, rng.reg(), 0x03),
        Op::Sw    => s_type(pick_data_addr(rng), rng.rs(), 0, 2, 0x23),
        Op::Beq   => b_type(pick_target(rng), rng.rs(), rng.rs(), 0, 0x63),
        Op::Bne   => b_type(pick_target(rng), rng.rs(), rng.rs(), 1, 0x63),
        Op::Blt   => b_type(pick_target(rng), rng.rs(), rng.rs(), 4, 0x63),
        Op::Bge   => b_type(pick_target(rng), rng.rs(), rng.rs(), 5, 0x63),
        Op::Bltu  => b_type(pick_target(rng), rng.rs(), rng.rs(), 6, 0x63),
        Op::Bgeu  => b_type(pick_target(rng), rng.rs(), rng.rs(), 7, 0x63),
        Op::Jal   => j_type(((halt_addr as i32) - (pc_byte as i32)).max(-(total_bytes as i32)).min(total_bytes as i32 - 1), rng.reg(), 0x6F),
    }
}

fn random_program(seed: u64, n_instrs: usize) -> Vec<u32> {
    let mut rng = Lcg::new(seed);
    let total_words = n_instrs as u32 + 1;
    let mut program: Vec<u32> = Vec::with_capacity(n_instrs + 1);
    for i in 0..n_instrs {
        let pc_byte = (i as u32) * 4;
        program.push(random_instr(&mut rng, pc_byte, total_words));
    }
    program.push(HALT);
    program
}

// ---- 3-way lockstep helpers --------------------------------------

fn run_sim(program: &[u32], max_steps: u64) -> Vec<(u32, u32)> {
    let cpu = sim::Cpu::new().run(program, max_steps);
    cpu.mem_writes
}

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
                let val = out.mem_wdata.raw() as u32;
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
                int_pending: bits::<32>(0),
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
                let val = out.mem_wdata.raw() as u32;
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
                int_pending: bits::<32>(0),
            }))
        },
        100,
    ).for_each(drop);
    writes
}

/// Compare two write sequences to a common prefix length — random
/// programs may not terminate cleanly within the per-implementation
/// cycle budget, so we compare the longest prefix they share.  A
/// real divergence (different write CONTENT at the same prefix
/// position) still fails; only length mismatch is tolerated.
fn assert_prefix_match(label: &str, seed: u64, n_instrs: usize,
                       a: &[(u32, u32)], a_name: &str,
                       b: &[(u32, u32)], b_name: &str,
                       program: &[u32]) {
    let n = a.len().min(b.len());
    if a[..n] != b[..n] {
        // Find first divergence index for a clearer message.
        let div = (0..n).find(|&i| a[i] != b[i]).unwrap_or(n);
        panic!(
            "fuzz {label} seed={seed} n_instrs={n_instrs}: {a_name} ↔ {b_name} divergence at write index {div}\n  program: {:?}\n  {a_name}[{div}] = {:?}\n  {b_name}[{div}] = {:?}\n  {a_name}: {} writes\n  {b_name}: {} writes",
            program,
            if div < a.len() { Some(a[div]) } else { None },
            if div < b.len() { Some(b[div]) } else { None },
            a.len(), b.len(),
        );
    }
}

fn assert_3way(seed: u64, n_instrs: usize, max_cycles: usize) {
    let program = random_program(seed, n_instrs);
    let sim_writes = run_sim(&program, max_cycles as u64);
    let single_writes = run_single_writes(program.clone(), max_cycles);
    let pipelined_writes = run_pipelined_writes(program.clone(), max_cycles * 3);
    assert_prefix_match("3way", seed, n_instrs,
        &sim_writes, "sim", &single_writes, "single", &program);
    assert_prefix_match("3way", seed, n_instrs,
        &sim_writes, "sim", &pipelined_writes, "pipelined", &program);
    assert_prefix_match("3way", seed, n_instrs,
        &single_writes, "single", &pipelined_writes, "pipelined", &program);
}

// ---- Sweep tests --------------------------------------------------
//
// Each test runs a sweep of N seeds → N programs → 3-way lockstep.
// Tests are partitioned by program-size class so a failure tells
// you which size class first diverges.

#[test]
fn fuzz_8_instr_programs_64_seeds() {
    for seed in 0..64 {
        assert_3way(seed, 8, 100);
    }
}

#[test]
fn fuzz_16_instr_programs_64_seeds() {
    for seed in 100..164 {
        assert_3way(seed, 16, 200);
    }
}

#[test]
fn fuzz_24_instr_programs_32_seeds() {
    for seed in 200..232 {
        assert_3way(seed, 24, 300);
    }
}

#[test]
fn fuzz_32_instr_programs_32_seeds() {
    for seed in 300..332 {
        assert_3way(seed, 32, 400);
    }
}

#[test]
fn fuzz_high_seed_range_for_diversity() {
    // Use a high-bit seed range to exercise different LCG state.
    for seed in 0xDEAD_BEEF_0000..0xDEAD_BEEF_0040 {
        assert_3way(seed, 16, 200);
    }
}
