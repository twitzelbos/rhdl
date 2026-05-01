//! CSR access and trap-vectoring tests for the single-cycle CPU.
//!
//! Per `tier-c-flagship-cores.md` §3.5 Phase 3.  Covers:
//!
//! - CSRRW write-then-read round-trip on `mscratch`.
//! - CSRRS bit-set semantics on `mstatus`.
//! - CSRRC bit-clear semantics on `mstatus`.
//! - CSRRWI / CSRRSI / CSRRCI immediate variants.
//! - Read-only CSRs (`misa`, `mhartid`) accept reads, drop writes.
//! - **ECALL traps**: `mepc` saves the PC, `mcause = 11`, PC vectors
//!   to `mtvec`.
//! - **EBREAK traps**: same shape, `mcause = 3`.
//!
//! All tests run on the single-cycle [`Cpu`] only.  Pipelined CSR
//! support is deferred to a follow-up — see PR #31's CHANGELOG.

use rhdl::prelude::*;
use rhdl_rv32i::cpu::{Cpu, In as SInIn, Out as SOut};
use rhdl_rv32i::csr::*;

// ---- Encoding helpers --------------------------------------------

fn r_type(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    (funct7 & 0x7F) << 25
        | (rs2 & 0x1F) << 20
        | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12
        | (rd & 0x1F) << 7
        | (opcode & 0x7F)
}

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

fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    i_type(imm, rs1, 0, rd, 0x13)
}
fn add(rd: u32, rs1: u32, rs2: u32) -> u32 {
    r_type(0, rs2, rs1, 0, rd, 0x33)
}
fn sw(rs2: u32, rs1: u32, imm: i32) -> u32 {
    s_type(imm, rs2, rs1, 2, 0x23)
}
fn lui(rd: u32, imm: u32) -> u32 {
    ((imm & 0xFFFF_F000)) | (rd & 0x1F) << 7 | 0x37
}

/// `beq x0, x0, +0` — infinite loop terminator.  Used to park the
/// CPU at the end of a test program so PC doesn't walk off the
/// end (which would now trigger an illegal-instruction trap and
/// overwrite mepc/mcause).
const HALT: u32 = 0x0000_0063;

// CSR instruction encodings.  All have opcode 0x73 and funct12 in
// bits [31:20] = the CSR address.
fn csrrw(rd: u32, rs1: u32, csr: u32) -> u32 {
    ((csr & 0xFFF) << 20) | (rs1 & 0x1F) << 15 | 1 << 12 | (rd & 0x1F) << 7 | 0x73
}
fn csrrs(rd: u32, rs1: u32, csr: u32) -> u32 {
    ((csr & 0xFFF) << 20) | (rs1 & 0x1F) << 15 | 2 << 12 | (rd & 0x1F) << 7 | 0x73
}
fn csrrc(rd: u32, rs1: u32, csr: u32) -> u32 {
    ((csr & 0xFFF) << 20) | (rs1 & 0x1F) << 15 | 3 << 12 | (rd & 0x1F) << 7 | 0x73
}
fn csrrwi(rd: u32, uimm: u32, csr: u32) -> u32 {
    ((csr & 0xFFF) << 20) | (uimm & 0x1F) << 15 | 5 << 12 | (rd & 0x1F) << 7 | 0x73
}
fn csrrsi(rd: u32, uimm: u32, csr: u32) -> u32 {
    ((csr & 0xFFF) << 20) | (uimm & 0x1F) << 15 | 6 << 12 | (rd & 0x1F) << 7 | 0x73
}
fn csrrci(rd: u32, uimm: u32, csr: u32) -> u32 {
    ((csr & 0xFFF) << 20) | (uimm & 0x1F) << 15 | 7 << 12 | (rd & 0x1F) << 7 | 0x73
}

const ECALL: u32 = 0x0000_0073;
const EBREAK: u32 = 0x0010_0073;

// ---- Closed-loop harness (same shape as tests/pipelined.rs but
// trimmed for single-cycle CPU; returns the final scratchpad).

fn run_cpu(program: Vec<u32>, max_cycles: usize) -> [u32; 256] {
    use rhdl::core::sim::ResetOrData;
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
            total_cycles += 1;
            if out.mem_write {
                let addr_word = (out.mem_addr.raw() / 4) as usize;
                if addr_word < data_mem.len() {
                    data_mem[addr_word] = out.mem_wdata.raw() as u32;
                }
            }
            let pc_word = (out.pc.raw() / 4) as usize;
            let instr: u32 = if pc_word < program.len() { program[pc_word] } else { 0 };
            let read_word = (out.mem_addr.raw() / 4) as usize;
            let mem_rdata: u32 = if read_word < data_mem.len() { data_mem[read_word] } else { 0 };
            Some(ResetOrData::Data(SInIn {
                instr: bits::<32>(instr as u128),
                mem_rdata: bits::<32>(mem_rdata as u128),
            }))
        },
        100,
    )
    .for_each(drop);
    data_mem
}

// ---- Tests --------------------------------------------------------

/// CSRRW: write rs1 to CSR, read CSR pre-modify into rd.
#[test]
fn csrrw_writes_then_reads_mscratch() {
    // addi x1, x0, 0x123      ; x1 = 0x123
    // csrrw x0, x1, mscratch  ; mscratch ← x1 (rd = x0 → discard read)
    // csrrw x2, x0, mscratch  ; x2 ← old mscratch = 0x123; mscratch ← 0
    // sw x2, 0(x0)            ; mem[0] = 0x123
    let program = vec![
        addi(1, 0, 0x123),
        csrrw(0, 1, CSR_MSCRATCH),
        csrrw(2, 0, CSR_MSCRATCH),
        sw(2, 0, 0),
    ];
    let mem = run_cpu(program, 16);
    assert_eq!(mem[0], 0x123);
}

/// CSRRS: set bits in CSR using rs1 as the bitmask, read pre-modify.
#[test]
fn csrrs_sets_bits_in_mstatus() {
    // addi x1, x0, 0xF0       ; x1 = 0xF0
    // csrrs x0, x1, mstatus   ; mstatus |= 0xF0
    // csrrs x2, x0, mstatus   ; x2 ← mstatus (no write since rs1 = x0)
    // sw x2, 0(x0)            ; mem[0] = 0xF0
    let program = vec![
        addi(1, 0, 0xF0),
        csrrs(0, 1, CSR_MSTATUS),
        csrrs(2, 0, CSR_MSTATUS),
        sw(2, 0, 0),
    ];
    let mem = run_cpu(program, 16);
    assert_eq!(mem[0], 0xF0);
}

/// CSRRC: clear bits in CSR using rs1 as the bitmask, read pre-modify.
#[test]
fn csrrc_clears_bits_in_mstatus() {
    // First set mstatus = 0xFF, then clear the low 4 bits.
    // addi x1, x0, 0xFF
    // csrrw x0, x1, mstatus   ; mstatus = 0xFF
    // addi x2, x0, 0x0F
    // csrrc x3, x2, mstatus   ; x3 ← 0xFF; mstatus &= ~0x0F = 0xF0
    // csrrs x4, x0, mstatus   ; x4 ← mstatus (no write)
    // sw x4, 0(x0)            ; mem[0] = 0xF0
    let program = vec![
        addi(1, 0, 0xFF),
        csrrw(0, 1, CSR_MSTATUS),
        addi(2, 0, 0x0F),
        csrrc(3, 2, CSR_MSTATUS),
        csrrs(4, 0, CSR_MSTATUS),
        sw(4, 0, 0),
    ];
    let mem = run_cpu(program, 16);
    assert_eq!(mem[0], 0xF0);
}

/// CSRRWI: zero-extended 5-bit immediate variant.
#[test]
fn csrrwi_writes_immediate() {
    // csrrwi x0, 0x1F, mscratch  ; mscratch = 0x1F
    // csrrs  x1, x0, mscratch    ; x1 ← 0x1F
    // sw     x1, 0(x0)           ; mem[0] = 0x1F
    let program = vec![
        csrrwi(0, 0x1F, CSR_MSCRATCH),
        csrrs(1, 0, CSR_MSCRATCH),
        sw(1, 0, 0),
    ];
    let mem = run_cpu(program, 12);
    assert_eq!(mem[0], 0x1F);
}

/// CSRRSI / CSRRCI: bit-set / bit-clear with 5-bit immediate.
#[test]
fn csrrsi_and_csrrci_modify_with_immediate() {
    // csrrwi x0, 0,   mscratch  ; mscratch = 0
    // csrrsi x0, 0x1A, mscratch ; mscratch |= 0x1A
    // csrrci x0, 0x02, mscratch ; mscratch &= ~0x02 = 0x18
    // csrrs  x1, x0,  mscratch  ; x1 ← 0x18
    // sw     x1, 0(x0)
    let program = vec![
        csrrwi(0, 0, CSR_MSCRATCH),
        csrrsi(0, 0x1A, CSR_MSCRATCH),
        csrrci(0, 0x02, CSR_MSCRATCH),
        csrrs(1, 0, CSR_MSCRATCH),
        sw(1, 0, 0),
    ];
    let mem = run_cpu(program, 16);
    assert_eq!(mem[0], 0x18);
}

/// Read-only `mhartid` returns 0; writes are silently dropped.
#[test]
fn mhartid_is_read_only_returns_zero() {
    // csrrw x0, x0, mhartid  ; "write" 0 (ignored anyway)
    // addi  x1, x0, 0x42
    // csrrw x2, x1, mhartid  ; "write" 0x42 (silently dropped); x2 ← 0
    // csrrs x3, x0, mhartid  ; x3 ← mhartid (still 0)
    // sw    x3, 0(x0)
    let program = vec![
        csrrw(0, 0, CSR_MHARTID),
        addi(1, 0, 0x42),
        csrrw(2, 1, CSR_MHARTID),
        csrrs(3, 0, CSR_MHARTID),
        sw(3, 0, 0),
    ];
    let mem = run_cpu(program, 16);
    assert_eq!(mem[0], 0);
}

/// Read-only `misa` returns the precomputed value (RV32I marker).
#[test]
fn misa_returns_constant_value() {
    // csrrs x1, x0, misa
    // sw    x1, 0(x0)
    let program = vec![
        csrrs(1, 0, CSR_MISA),
        sw(1, 0, 0),
    ];
    let mem = run_cpu(program, 8);
    assert_eq!(mem[0], 0x4000_0100, "misa should be RV32I marker");
}

/// ECALL trap: PC vectors to mtvec, mepc holds the trapping PC,
/// mcause = 11 (M-mode environment call).
#[test]
fn ecall_traps_to_mtvec_with_correct_mepc_and_mcause() {
    // 0x00: lui   x1, 0x10        ; x1 = 0x10000  (trap-vector address)
    // 0x04: csrrw x0, x1, mtvec   ; mtvec = 0x10000
    // 0x08: ecall                  ; trap → PC = mtvec, mepc = 0x08, mcause = 11
    // 0x0C: addi  x2, x0, 0x77    ; SHOULD NOT EXECUTE (squashed by trap)
    // 0x10: sw    x2, 0(x0)       ; SHOULD NOT EXECUTE
    //
    // After the trap, the CPU is at PC = 0x10000 — beyond our
    // program memory.  The harness returns 0s for instr fetches
    // past the end (treated as illegal/NOP).  We observe trap
    // success by reading back mepc and mcause via CSR after the
    // trap fires.  But the post-trap PC won't run our cleanup
    // code — so instead let's place the trap handler at 0x10
    // (small mtvec value):
    //
    // 0x00: lui   x1, 0           ; x1 = 0
    // 0x04: addi  x1, x1, 0x10    ; x1 = 0x10
    // 0x08: csrrw x0, x1, mtvec   ; mtvec = 0x10
    // 0x0C: ecall                 ; trap → PC = 0x10, mepc = 0x0C, mcause = 11
    // 0x10: csrrs x2, x0, mepc    ; x2 ← 0x0C  (handler reads mepc)
    // 0x14: csrrs x3, x0, mcause  ; x3 ← 11
    // 0x18: sw    x2, 0(x0)       ; mem[0] = 0x0C (saved PC)
    // 0x1C: sw    x3, 4(x0)       ; mem[1] = 11   (cause)
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x10),
        csrrw(0, 1, CSR_MTVEC),
        ECALL,
        csrrs(2, 0, CSR_MEPC),
        csrrs(3, 0, CSR_MCAUSE),
        sw(2, 0, 0),
        sw(3, 0, 4),
        HALT,                  // park CPU so PC doesn't fall off the end
    ];
    let mem = run_cpu(program, 24);
    assert_eq!(mem[0], 0x0C, "mepc should hold the ECALL's PC");
    assert_eq!(mem[1], 11, "mcause should be 11 (M-mode ECALL)");
}

/// EBREAK trap: same shape as ECALL, mcause = 3 (Breakpoint).
#[test]
fn ebreak_traps_with_cause_3() {
    // Setup: mtvec = 0x10, then EBREAK at 0x0C.
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x10),
        csrrw(0, 1, CSR_MTVEC),
        EBREAK,
        csrrs(2, 0, CSR_MEPC),
        csrrs(3, 0, CSR_MCAUSE),
        sw(2, 0, 0),
        sw(3, 0, 4),
        HALT,                  // park CPU
    ];
    let mem = run_cpu(program, 24);
    assert_eq!(mem[0], 0x0C, "mepc should hold the EBREAK's PC");
    assert_eq!(mem[1], 3, "mcause should be 3 (Breakpoint)");
}

/// CSR-instruction writeback to rd: `rd ← old CSR value`, atomically.
#[test]
fn csrrw_returns_old_value_to_rd_when_rd_nonzero() {
    // mscratch starts at 0.
    // addi  x1, x0, 0xAA
    // csrrw x0, x1, mscratch    ; mscratch = 0xAA (rd = x0 → no observed write)
    // addi  x2, x0, 0xBB
    // csrrw x3, x2, mscratch    ; x3 ← old mscratch = 0xAA; mscratch = 0xBB
    // sw    x3, 0(x0)            ; mem[0] = 0xAA
    let program = vec![
        addi(1, 0, 0xAA - 0x100),  // 0xAA fits in -1...-128 sign-extended? No, 0xAA = 170, > 127. Use addi twice.
        addi(1, 0, 0x55),  // x1 = 0x55
        add(1, 1, 1),      // x1 = 0xAA (= 0x55 + 0x55)
        csrrw(0, 1, CSR_MSCRATCH),
        addi(2, 0, 0x5D),
        add(2, 2, 2),      // x2 = 0xBA
        csrrw(3, 2, CSR_MSCRATCH),
        sw(3, 0, 0),
    ];
    let mem = run_cpu(program, 20);
    assert_eq!(mem[0], 0xAA);
}

/// Iverilog round-trip on the CPU with the new CSR file.
#[test]
fn cpu_with_csrs_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut = Cpu::default();
    let inputs: Vec<SInIn> = (0..6)
        .map(|cycle| SInIn {
            instr: bits::<32>(match cycle {
                0 => addi(1, 0, 0x42) as u128,
                1 => csrrw(0, 1, CSR_MSCRATCH) as u128,
                2 => csrrs(2, 0, CSR_MSCRATCH) as u128,
                3 => sw(2, 0, 0) as u128,
                _ => 0,
            }),
            mem_rdata: bits::<32>(0),
        })
        .collect();
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
