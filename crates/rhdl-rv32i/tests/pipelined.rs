//! 5-stage pipelined CPU tests.
//!
//! The validation strategy per `tier-c-flagship-cores.md` §3.5 is
//! **byte-identical parity against the single-cycle reference**.
//! Each test runs the same program through both
//! [`rhdl_rv32i::cpu::Cpu`] (single-cycle, the executable spec)
//! and [`rhdl_rv32i::pipelined::PipelinedCpu`] (the 5-stage core),
//! then compares the **architectural state observed via store-word**
//! at the end.
//!
//! We compare the result via memory writes rather than peeking at
//! internal regfile state because that's what real software sees:
//! the user-visible value of a register at the end of a program is
//! whatever the program last stored to memory, full stop.
//!
//! ## What v0.2 covers
//!
//! - Pure ALU programs (no hazards) — parity check.
//! - Programs with back-to-back ALU dependencies — exercises
//!   forwarding from EX/MEM and MEM/WB.
//! - Programs with load-use hazards — exercises the 1-cycle stall.
//! - JAL — exercises the unconditional branch squash.
//!
//! ## What v0.2 does NOT yet validate
//!
//! - Conditional branches across many control-flow patterns.
//! - JALR with non-trivial targets.
//! - Misaligned-target traps (RV32I requires these; v0.1 single-
//!   cycle silently masks).
//!
//! These edge-case tests are deferred to follow-up PRs along with
//! the riscv-tests harness per the cross-cutting validation plan.

use rhdl::prelude::*;
use rhdl_rv32i::cpu::{Cpu, In as SInIn, Out as SOut};
use rhdl_rv32i::pipelined::{In as PIn, Out as POut, PipelinedCpu};

// ---- Encoding helpers (same as in tests/cpu.rs, factored locally
// to keep the test file self-contained).

fn r(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    (funct7 & 0x7F) << 25
        | (rs2 & 0x1F) << 20
        | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12
        | (rd & 0x1F) << 7
        | (opcode & 0x7F)
}

fn i(imm: i32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    let imm_u = (imm as u32) & 0xFFF;
    (imm_u << 20)
        | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12
        | (rd & 0x1F) << 7
        | (opcode & 0x7F)
}

fn s(imm: i32, rs2: u32, rs1: u32, funct3: u32, opcode: u32) -> u32 {
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
    i(imm, rs1, 0, rd, 0x13)
}
#[allow(dead_code)]
fn add(rd: u32, rs1: u32, rs2: u32) -> u32 {
    r(0, rs2, rs1, 0, rd, 0x33)
}
#[allow(dead_code)]
fn sub(rd: u32, rs1: u32, rs2: u32) -> u32 {
    r(0x20, rs2, rs1, 0, rd, 0x33)
}
fn sw(rs2: u32, rs1: u32, imm: i32) -> u32 {
    s(imm, rs2, rs1, 2, 0x23)
}
fn lw(rd: u32, rs1: u32, imm: i32) -> u32 {
    i(imm, rs1, 2, rd, 0x03)
}
fn jal(rd: u32, imm: i32) -> u32 {
    let imm_u = (imm as u32) & 0x1F_FFFF;
    let bit20 = (imm_u >> 20) & 1;
    let bits_19_12 = (imm_u >> 12) & 0xFF;
    let bit11 = (imm_u >> 11) & 1;
    let bits_10_1 = (imm_u >> 1) & 0x3FF;
    (bit20 << 31)
        | bits_10_1 << 21
        | bit11 << 20
        | bits_19_12 << 12
        | (rd & 0x1F) << 7
        | 0x6F
}

// ---- Program-driver harnesses ------------------------------------

/// Closed-loop run harness — drives the program memory based on
/// the CPU's actual `pc` output (so stalls and branch redirects
/// are handled correctly), and drives data memory from a static
/// 256-word scratchpad.  Stores update the scratchpad in place;
/// loads return the scratchpad's current value at the requested
/// address.
///
/// Returns the scratchpad's contents at the end of the run, so
/// the caller can assert on whichever word the program just
/// stored to.
fn run_until<S, P>(uut: &S, program: &[u32], max_cycles: usize) -> [u32; 256]
where
    S: rhdl::prelude::Synchronous<S = P>,
    S: SynchronousIO,
    <S as SynchronousIO>::I: ProgramIo,
    <S as SynchronousIO>::O: ProgramObservable + Clone,
{
    use rhdl::core::sim::ResetOrData;

    let prog: Vec<u32> = program.to_vec();
    let mut data_mem: [u32; 256] = [0; 256];
    let mut reset_cycles_remaining = 2;
    let mut total_cycles: usize = 0;

    uut.run_fn(
        |out| {
            if reset_cycles_remaining > 0 {
                reset_cycles_remaining -= 1;
                return Some(ResetOrData::Reset);
            }
            if total_cycles >= max_cycles {
                return None;
            }
            total_cycles += 1;

            // Apply any in-flight store from this cycle's output to
            // the data scratchpad.
            if out.mem_write_b() {
                let addr_word = (out.mem_addr_b() / 4) as usize;
                if addr_word < data_mem.len() {
                    data_mem[addr_word] = out.mem_wdata_b();
                }
            }

            // Fetch the next instruction based on the CPU's PC.
            let pc_word = (out.pc_b() / 4) as usize;
            let instr: u32 = if pc_word < prog.len() {
                prog[pc_word]
            } else {
                0
            };
            // Drive data-memory read based on the CPU's mem_addr.
            let read_word = (out.mem_addr_b() / 4) as usize;
            let mem_rdata: u32 = if read_word < data_mem.len() {
                data_mem[read_word]
            } else {
                0
            };

            Some(ResetOrData::Data(<S as SynchronousIO>::I::from_parts(
                instr, mem_rdata,
            )))
        },
        100,
    )
    .for_each(drop);

    data_mem
}

/// Adapter trait — both single-cycle and pipelined `In` types
/// have the same shape (`instr` + `mem_rdata`); we provide a
/// constructor so the closed-loop harness is generic over the
/// CPU.
trait ProgramIo {
    fn from_parts(instr: u32, mem_rdata: u32) -> Self;
}

impl ProgramIo for SInIn {
    fn from_parts(instr: u32, mem_rdata: u32) -> Self {
        Self {
            instr: bits::<32>(instr as u128),
            mem_rdata: bits::<32>(mem_rdata as u128),
        }
    }
}

impl ProgramIo for PIn {
    fn from_parts(instr: u32, mem_rdata: u32) -> Self {
        Self {
            instr: bits::<32>(instr as u128),
            mem_rdata: bits::<32>(mem_rdata as u128),
        }
    }
}

/// Adapter trait for reading the few output fields the harness
/// needs from either CPU's `Out` type.
trait ProgramObservable {
    fn pc_b(&self) -> u32;
    fn mem_addr_b(&self) -> u32;
    fn mem_wdata_b(&self) -> u32;
    fn mem_write_b(&self) -> bool;
}

impl ProgramObservable for SOut {
    fn pc_b(&self) -> u32 { self.pc.raw() as u32 }
    fn mem_addr_b(&self) -> u32 { self.mem_addr.raw() as u32 }
    fn mem_wdata_b(&self) -> u32 { self.mem_wdata.raw() as u32 }
    fn mem_write_b(&self) -> bool { self.mem_write }
}

impl ProgramObservable for POut {
    fn pc_b(&self) -> u32 { self.pc.raw() as u32 }
    fn mem_addr_b(&self) -> u32 { self.mem_addr.raw() as u32 }
    fn mem_wdata_b(&self) -> u32 { self.mem_wdata.raw() as u32 }
    fn mem_write_b(&self) -> bool { self.mem_write }
}

fn run_single(program: &[u32], cycles: usize) -> [u32; 256] {
    run_until(&Cpu::default(), program, cycles)
}

fn run_pipelined(program: &[u32], cycles: usize) -> [u32; 256] {
    run_until(&PipelinedCpu::default(), program, cycles)
}

// ---- Tests --------------------------------------------------------

/// Pure-ALU program: no hazards, no branches.  Pipelined and
/// single-cycle should produce the same final scratchpad state.
#[test]
fn pipelined_pure_alu_parity() {
    let program = vec![
        addi(1, 0, 10),
        addi(2, 0, 20),
        addi(3, 0, 30),
        addi(4, 0, 40),
        addi(5, 0, 50),
        sw(5, 0, 0),       // mem[0] = x5 = 50
    ];
    let single = run_single(&program, 16);
    let pipelined = run_pipelined(&program, 24);
    assert_eq!(single[0], 50);
    assert_eq!(pipelined[0], single[0], "pipelined should match single-cycle");
}

/// Hazard test: back-to-back ALU dependency.  Pipelined needs
/// EX/MEM forwarding to produce the right answer in the second
/// instruction.
#[test]
fn pipelined_back_to_back_dependency_uses_forwarding() {
    let program = vec![
        addi(1, 0, 5),
        addi(2, 1, 7),     // depends on x1 from previous cycle
        sw(2, 0, 0),       // mem[0] = x2 = 12
    ];
    let single = run_single(&program, 12);
    let pipelined = run_pipelined(&program, 20);
    assert_eq!(single[0], 12);
    assert_eq!(pipelined[0], single[0], "EX/MEM forwarding should match");
}

/// Three-deep dependency chain — exercises forwarding from
/// MEM/WB (the 2-instructions-back source).
#[test]
fn pipelined_three_deep_chain_uses_mem_wb_forwarding() {
    let program = vec![
        addi(1, 0, 100),
        addi(2, 1, 1),     // x2 = 101 (EX/MEM fwd)
        addi(3, 1, 200),   // x3 = 300 (MEM/WB fwd)
        sw(3, 0, 0),       // mem[0] = x3 = 300
    ];
    let single = run_single(&program, 12);
    let pipelined = run_pipelined(&program, 20);
    assert_eq!(single[0], 300);
    assert_eq!(pipelined[0], single[0], "MEM/WB forwarding should match");
}

/// Load-use hazard: a load followed immediately by an ADDI on the
/// loaded value.  The pipelined core stalls 1 cycle, then
/// forwards from MEM/WB.
///
/// The closed-loop harness updates the data scratchpad in real time,
/// so we pre-seed mem[1] = 42 by storing it at the start of the
/// program.  Then the LW reads mem[1].
#[test]
fn pipelined_load_use_stall_inserts_bubble() {
    let program = vec![
        addi(7, 0, 42),    // x7 = 42
        sw(7, 0, 4),       // mem[1] = 42
        lw(1, 0, 4),       // x1 = mem[1] = 42
        addi(2, 1, 1),     // x2 = 43 (after stall + MEM/WB forward)
        sw(2, 0, 8),       // mem[2] = x2 = 43
    ];
    let single = run_single(&program, 16);
    let pipelined = run_pipelined(&program, 30);
    assert_eq!(single[2], 43);
    assert_eq!(pipelined[2], single[2], "load-use stall + forward should match");
}

/// JAL: unconditional jump.  Pipelined squashes the next
/// instruction and redirects.
#[test]
fn pipelined_jal_squashes_and_redirects() {
    let program = vec![
        jal(1, 12),                // PC ← 0x0C
        addi(2, 0, 999),           // SQUASHED
        addi(3, 0, 999),           // SQUASHED
        addi(4, 0, 7),             // x4 = 7
        sw(4, 0, 0),               // mem[0] = 7
    ];
    let single = run_single(&program, 12);
    let pipelined = run_pipelined(&program, 24);
    assert_eq!(single[0], 7, "single-cycle JAL skips x2 and x3");
    assert_eq!(pipelined[0], single[0], "pipelined JAL should also skip");
}

/// Iverilog round-trip on the pipelined CPU — the load-bearing
/// "the lowering doesn't break" check.
#[test]
fn pipelined_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut = PipelinedCpu::default();
    let program = vec![
        addi(1, 0, 5),
        addi(2, 1, 7),
        sw(2, 0, 0),
    ];
    let inputs: Vec<PIn> = (0..12)
        .map(|cycle| PIn {
            instr: bits::<32>(if cycle < program.len() {
                program[cycle] as u128
            } else {
                0
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
