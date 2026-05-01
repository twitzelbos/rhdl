//! Pipelined CSR + trap tests.
//!
//! Mirror of the single-cycle `csr_trap.rs` tests, validated
//! against the pipelined CPU.  Each test runs the same program
//! through both cores and asserts byte-identical scratchpad
//! agreement (the parity contract from PR #31 + #32).
//!
//! Closes the Phase 3 pipelined gap from PR #33's CHANGELOG.

use rhdl::core::sim::ResetOrData;
use rhdl::prelude::*;
use rhdl_rv32i::cpu::{Cpu, In as SInIn, Out as SOut};
use rhdl_rv32i::csr::*;
use rhdl_rv32i::pipelined::{In as PIn, Out as POut, PipelinedCpu};

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

fn addi(rd: u32, rs1: u32, imm: i32) -> u32 { i_type(imm, rs1, 0, rd, 0x13) }
#[allow(dead_code)]
fn add(rd: u32, rs1: u32, rs2: u32) -> u32  { r_type(0, rs2, rs1, 0, rd, 0x33) }
fn sw(rs2: u32, rs1: u32, imm: i32) -> u32  { s_type(imm, rs2, rs1, 2, 0x23) }
fn lui(rd: u32, imm20: u32) -> u32          { (imm20 & 0xFFFFF) << 12 | (rd & 0x1F) << 7 | 0x37 }

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

const ECALL: u32 = 0x0000_0073;
const EBREAK: u32 = 0x0010_0073;
/// `beq x0, x0, +0` infinite-loop terminator.
const HALT: u32 = 0x0000_0063;

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

/// Parity helper: assert single-cycle and pipelined agree on the
/// first 4 scratchpad words.
fn assert_parity(program: Vec<u32>, single_cycles: usize, pipelined_cycles: usize, name: &str) {
    let single = run_single(program.clone(), single_cycles);
    let pipelined = run_pipelined(program, pipelined_cycles);
    for i in 0..4 {
        assert_eq!(
            pipelined[i], single[i],
            "{name}: scratchpad word {i} differs (single={} pipelined={})",
            single[i], pipelined[i],
        );
    }
}

// ---- Tests --------------------------------------------------------

#[test]
fn pipelined_csrrw_round_trip_parity() {
    let program = vec![
        addi(1, 0, 0x123),
        csrrw(0, 1, CSR_MSCRATCH),
        addi(0, 0, 0),                 // NOP padding to ensure CSR write commits
        addi(0, 0, 0),
        csrrw(2, 0, CSR_MSCRATCH),
        sw(2, 0, 0),
    ];
    assert_parity(program, 16, 30, "csrrw_round_trip");
}

#[test]
fn pipelined_csrrs_set_bits_parity() {
    let program = vec![
        addi(1, 0, 0xF0),
        csrrw(0, 1, CSR_MSTATUS),       // mstatus = 0xF0
        addi(0, 0, 0),
        addi(0, 0, 0),
        csrrs(2, 0, CSR_MSTATUS),       // x2 = mstatus
        sw(2, 0, 0),
    ];
    assert_parity(program, 16, 30, "csrrs_set_bits");
}

#[test]
fn pipelined_csrrc_clear_bits_parity() {
    let program = vec![
        addi(1, 0, 0xFF),
        csrrw(0, 1, CSR_MSTATUS),
        addi(2, 0, 0x0F),
        addi(0, 0, 0),                  // padding so the write commits
        csrrc(3, 2, CSR_MSTATUS),       // mstatus &= ~0x0F → 0xF0
        addi(0, 0, 0),
        addi(0, 0, 0),
        csrrs(4, 0, CSR_MSTATUS),
        sw(4, 0, 0),
    ];
    assert_parity(program, 24, 40, "csrrc_clear_bits");
}

#[test]
fn pipelined_csrrwi_immediate_parity() {
    let program = vec![
        csrrwi(0, 0x1F, CSR_MSCRATCH),
        addi(0, 0, 0),
        addi(0, 0, 0),
        csrrs(1, 0, CSR_MSCRATCH),
        sw(1, 0, 0),
    ];
    assert_parity(program, 12, 24, "csrrwi_immediate");
}

#[test]
fn pipelined_mhartid_read_only_parity() {
    let program = vec![
        addi(1, 0, 0x42),
        csrrw(2, 1, CSR_MHARTID),       // attempted write dropped; read returns 0
        addi(0, 0, 0),
        addi(0, 0, 0),
        csrrs(3, 0, CSR_MHARTID),
        sw(3, 0, 0),
    ];
    assert_parity(program, 16, 30, "mhartid_read_only");
}

#[test]
fn pipelined_misa_constant_parity() {
    let program = vec![
        csrrs(1, 0, CSR_MISA),
        addi(0, 0, 0),
        addi(0, 0, 0),
        sw(1, 0, 0),
    ];
    assert_parity(program, 12, 24, "misa_constant");
}

#[test]
fn pipelined_ecall_trap_parity() {
    // Setup: mtvec = 0x20, ECALL at 0x14.  Trap handler at 0x20
    // reads mepc and mcause, writes them to scratchpad.
    let program = vec![
        lui(1, 0),                      // 0x00: x1 = 0
        addi(1, 1, 0x20),               // 0x04: x1 = 0x20
        csrrw(0, 1, CSR_MTVEC),         // 0x08: mtvec = 0x20
        addi(0, 0, 0),                  // 0x0C: NOP (ensure mtvec commits)
        addi(0, 0, 0),                  // 0x10: NOP
        ECALL,                          // 0x14: trap → 0x20, mepc = 0x14, mcause = 11
        addi(0, 0, 0),                  // 0x18: SQUASHED
        addi(0, 0, 0),                  // 0x1C: SQUASHED
        // 0x20: trap handler
        csrrs(2, 0, CSR_MEPC),
        csrrs(3, 0, CSR_MCAUSE),
        sw(2, 0, 0),
        sw(3, 0, 4),
        HALT,                            // park CPU
    ];
    let single = run_single(program.clone(), 24);
    let pipelined = run_pipelined(program, 50);
    assert_eq!(single[0], 0x14, "single-cycle: mepc should be 0x14");
    assert_eq!(single[1], 11, "single-cycle: mcause should be 11");
    assert_eq!(pipelined[0], single[0], "pipelined: mepc parity");
    assert_eq!(pipelined[1], single[1], "pipelined: mcause parity");
}

#[test]
fn pipelined_ebreak_trap_parity() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        addi(0, 0, 0),
        addi(0, 0, 0),
        EBREAK,                         // 0x14
        addi(0, 0, 0),
        addi(0, 0, 0),
        // 0x20: handler
        csrrs(2, 0, CSR_MEPC),
        csrrs(3, 0, CSR_MCAUSE),
        sw(2, 0, 0),
        sw(3, 0, 4),
        HALT,
    ];
    let single = run_single(program.clone(), 24);
    let pipelined = run_pipelined(program, 50);
    assert_eq!(single[0], 0x14, "single-cycle: mepc should be 0x14");
    assert_eq!(single[1], 3, "single-cycle: mcause should be 3");
    assert_eq!(pipelined[0], single[0], "pipelined: mepc parity");
    assert_eq!(pipelined[1], single[1], "pipelined: mcause parity");
}

#[test]
fn pipelined_iverilog_round_trip_with_csrs() -> Result<(), RHDLError> {
    let uut = PipelinedCpu::default();
    let inputs: Vec<PIn> = (0..10)
        .map(|cycle| PIn {
            instr: bits::<32>(match cycle {
                0 => addi(1, 0, 0x42) as u128,
                1 => csrrw(0, 1, CSR_MSCRATCH) as u128,
                2 => addi(0, 0, 0) as u128,
                3 => addi(0, 0, 0) as u128,
                4 => csrrs(2, 0, CSR_MSCRATCH) as u128,
                5 => sw(2, 0, 0) as u128,
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
