//! MRET (return-from-trap) and illegal-instruction trap tests.
//!
//! Both behaviours run on both cores and assert byte-identical
//! parity on the resulting scratchpad.
//!
//! ## What's tested
//!
//! - **Trap-then-return round-trip**: ECALL traps to handler;
//!   handler increments mepc by 4 (so MRET resumes at the
//!   instruction *after* the ECALL); MRET returns to user code;
//!   user code stores a marker proving execution resumed.
//! - **Illegal-instruction trap**: an instruction with an
//!   unrecognized opcode (e.g. all-zero in user's program) traps
//!   with mcause = 2 and PC vectors to mtvec.
//! - Both behaviours validated against the single-cycle
//!   reference for both single-cycle and pipelined cores.

use rhdl::core::sim::ResetOrData;
use rhdl::prelude::*;
use rhdl_rv32i::cpu::{Cpu, In as SInIn, Out as SOut};
use rhdl_rv32i::csr::*;
use rhdl_rv32i::pipelined::{In as PIn, Out as POut, PipelinedCpu};

// ---- Encoding helpers (same shape as csr_trap.rs) ----------------

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

const ECALL: u32  = 0x0000_0073;
const MRET: u32   = 0x3020_0073;       // funct12 = 0x302, opcode = 0x73
const HALT: u32   = 0x0000_0063;       // beq x0, x0, +0
const ILLEGAL: u32 = 0x0000_007F;      // opcode 0x7F is unassigned in RV32I

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

// ---- Tests --------------------------------------------------------

/// Single-cycle: ECALL → handler → MRET → user-code resumes.
///
/// Handler:
///   1. Read mepc (= ECALL's PC).
///   2. Compute mepc + 4 (skip the ECALL instruction so we don't
///      loop forever).
///   3. Write back to mepc.
///   4. MRET → PC ← mepc, returns to user code.
///
/// User code post-MRET stores a marker.  Test asserts the marker
/// is present, proving MRET worked.
#[test]
fn mret_returns_to_user_code_single_cycle() {
    // 0x00: lui   x1, 0
    // 0x04: addi  x1, x1, 0x20         ; x1 = 0x20 (handler addr)
    // 0x08: csrrw x0, x1, mtvec        ; mtvec = 0x20
    // 0x0C: addi  x0, x0, 0            ; NOP (commit mtvec)
    // 0x10: addi  x0, x0, 0            ; NOP
    // 0x14: ecall                       ; trap → handler at 0x20
    // 0x18: addi  x10, x0, 0xAB        ; user code post-MRET
    // 0x1C: sw    x10, 0(x0)           ; mem[0] = 0xAB (proof!)
    // 0x20: HALT
    //  ...handler at 0x24
    // 0x24: csrrs x2, x0, mepc         ; x2 ← 0x14
    // 0x28: addi  x2, x2, 4            ; x2 = 0x18 (next instr)
    // 0x2C: csrrw x0, x2, mepc         ; mepc = 0x18
    // 0x30: mret                       ; PC ← mepc = 0x18
    //
    // To avoid colliding with the trap-to-handler vectoring, set
    // mtvec = 0x24 (the handler entry) instead of 0x20.
    let program = vec![
        lui(1, 0),                     // 0x00
        addi(1, 1, 0x24),              // 0x04
        csrrw(0, 1, CSR_MTVEC),        // 0x08
        addi(0, 0, 0),                 // 0x0C
        addi(0, 0, 0),                 // 0x10
        ECALL,                         // 0x14
        addi(10, 0, 0xAB),             // 0x18: post-MRET landing
        sw(10, 0, 0),                  // 0x1C: mem[0] = 0xAB
        HALT,                          // 0x20: park
        // 0x24: handler
        csrrs(2, 0, CSR_MEPC),         // 0x24: x2 ← mepc
        addi(2, 2, 4),                 // 0x28: x2 += 4
        csrrw(0, 2, CSR_MEPC),         // 0x2C: mepc = x2
        MRET,                          // 0x30: return → PC = 0x18
    ];
    let mem = run_single(program, 36);
    assert_eq!(
        mem[0], 0xAB,
        "MRET should return to user code; expected mem[0] = 0xAB",
    );
}

/// Pipelined: same program shape with NOPs between the CSRRW
/// updating mepc and the MRET, so the CSRRW's write commits
/// before MRET reads mepc.
///
/// (v0.7 doesn't have same-cycle CSR-to-CSR forwarding; the
/// pipelined CPU needs the writeback to commit before the next
/// CSR-read can see it.  Documented in PR #35's CHANGELOG.)
#[test]
fn mret_returns_to_user_code_pipelined_parity() {
    // Same program as the single-cycle test but with the handler
    // expanded with NOPs.  Handler at 0x28 (was 0x24) — adjust mtvec.
    let program = vec![
        lui(1, 0),                     // 0x00
        addi(1, 1, 0x28),              // 0x04
        csrrw(0, 1, CSR_MTVEC),        // 0x08
        addi(0, 0, 0),                 // 0x0C
        addi(0, 0, 0),                 // 0x10
        ECALL,                         // 0x14
        addi(10, 0, 0xAB),             // 0x18
        sw(10, 0, 0),                  // 0x1C
        HALT,                          // 0x20
        addi(0, 0, 0),                 // 0x24: pad
        // 0x28: handler
        csrrs(2, 0, CSR_MEPC),         // 0x28
        addi(2, 2, 4),                 // 0x2C
        csrrw(0, 2, CSR_MEPC),         // 0x30
        addi(0, 0, 0),                 // 0x34: NOP — wait for CSRRW to commit
        addi(0, 0, 0),                 // 0x38: NOP
        addi(0, 0, 0),                 // 0x3C: NOP
        MRET,                          // 0x40: PC ← mepc (now 0x18)
    ];
    let single = run_single(program.clone(), 50);
    let pipelined = run_pipelined(program, 90);
    assert_eq!(single[0], 0xAB, "single-cycle: mem[0] should be 0xAB");
    assert_eq!(
        pipelined[0], single[0],
        "pipelined MRET parity should match single-cycle",
    );
}

/// Illegal-instruction trap: opcode 0x7F (unassigned in RV32I)
/// triggers a trap with mcause = 2.  Handler reads mepc + mcause
/// and stores them.
#[test]
fn illegal_instruction_traps_with_cause_2_single_cycle() {
    // 0x00: setup mtvec = 0x20
    // 0x14: ILLEGAL instruction → trap → handler at 0x20
    // 0x20: handler reads mepc + mcause, stores them
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        addi(0, 0, 0),
        addi(0, 0, 0),
        ILLEGAL,                       // 0x14: traps with cause = 2
        addi(0, 0, 0),                 // SQUASHED
        addi(0, 0, 0),                 // SQUASHED
        // 0x20: handler
        csrrs(2, 0, CSR_MEPC),
        csrrs(3, 0, CSR_MCAUSE),
        sw(2, 0, 0),
        sw(3, 0, 4),
        HALT,
    ];
    let mem = run_single(program, 24);
    assert_eq!(mem[0], 0x14, "mepc should hold the illegal-instr's PC");
    assert_eq!(mem[1], 2, "mcause should be 2 (Illegal instruction)");
}

#[test]
fn illegal_instruction_traps_pipelined_parity() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        addi(0, 0, 0),
        addi(0, 0, 0),
        ILLEGAL,
        addi(0, 0, 0),
        addi(0, 0, 0),
        csrrs(2, 0, CSR_MEPC),
        csrrs(3, 0, CSR_MCAUSE),
        sw(2, 0, 0),
        sw(3, 0, 4),
        HALT,
    ];
    let single = run_single(program.clone(), 24);
    let pipelined = run_pipelined(program, 50);
    assert_eq!(single[0], 0x14, "single-cycle: mepc");
    assert_eq!(single[1], 2, "single-cycle: mcause = 2 (Illegal)");
    assert_eq!(pipelined[0], single[0], "pipelined: mepc parity");
    assert_eq!(pipelined[1], single[1], "pipelined: mcause parity");
}

#[test]
fn mret_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut = PipelinedCpu::default();
    let inputs: Vec<PIn> = (0..14)
        .map(|cycle| PIn {
            instr: bits::<32>(match cycle {
                0 => lui(1, 0) as u128,
                1 => addi(1, 1, 0x20) as u128,
                2 => csrrw(0, 1, CSR_MTVEC) as u128,
                3 => addi(0, 0, 0) as u128,
                4 => addi(0, 0, 0) as u128,
                5 => ECALL as u128,
                6 => addi(10, 0, 0xAB) as u128,
                7 => sw(10, 0, 0) as u128,
                8 => HALT as u128,
                9 => csrrs(2, 0, CSR_MEPC) as u128,
                10 => addi(2, 2, 4) as u128,
                11 => csrrw(0, 2, CSR_MEPC) as u128,
                12 => MRET as u128,
                _ => HALT as u128,
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
