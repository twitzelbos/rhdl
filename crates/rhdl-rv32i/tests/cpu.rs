//! Single-cycle CPU end-to-end tests.
//!
//! Each test wires the [`Cpu`] up to a fixed program memory (an
//! array of pre-encoded instructions) and a small data scratchpad,
//! runs it for N cycles, and asserts on the final register-file
//! state via memory reads.
//!
//! v0.1 of `rhdl-rv32i` exposes the program memory and data memory
//! as combinational inputs that the parent (the test harness)
//! drives based on the CPU's `pc` and `mem_addr` outputs.  The
//! harness here implements a simple stub for that.

use rhdl::prelude::*;
use rhdl_rv32i::cpu::*;

/// Encode an R-type instruction.
fn r(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    (funct7 & 0x7F) << 25
        | (rs2 & 0x1F) << 20
        | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12
        | (rd & 0x1F) << 7
        | (opcode & 0x7F)
}

/// Encode an I-type instruction.
fn i(imm: i32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    let imm_u = (imm as u32) & 0xFFF;
    (imm_u << 20)
        | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12
        | (rd & 0x1F) << 7
        | (opcode & 0x7F)
}

/// Encode a U-type instruction.
fn u(imm: u32, rd: u32, opcode: u32) -> u32 {
    (imm & 0xFFFF_F000) | (rd & 0x1F) << 7 | (opcode & 0x7F)
}

// Common instruction helpers (RV32I encoding shortcuts).

fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    i(imm, rs1, 0, rd, 0x13)
}

fn add(rd: u32, rs1: u32, rs2: u32) -> u32 {
    r(0, rs2, rs1, 0, rd, 0x33)
}

fn lui(rd: u32, imm: u32) -> u32 {
    // imm here is the upper 20-bit field placed at bits [31:12].
    u(imm << 12, rd, 0x37)
}

fn sub(rd: u32, rs1: u32, rs2: u32) -> u32 {
    r(0x20, rs2, rs1, 0, rd, 0x33)
}

/// Run a fixed program for `cycles` cycles and return the
/// post-reset trace of (pc, mem_addr, mem_write, mem_wdata) per
/// cycle.  Program memory is the slice; data memory always reads
/// zero (no loads in these tests).
fn run_program(program: Vec<u32>, cycles: usize) -> Vec<(Bits<32>, Out)> {
    let uut = Cpu::default();

    // Build the input stream by closure: each cycle's input is
    // computed from the previous cycle's output (specifically the
    // PC).  Since we don't have access to the running CPU's
    // outputs from outside, we use `run_fn` if available.  The
    // simpler path: pre-compute inputs assuming PC advances by 4
    // each cycle (no branches/jumps in the tested programs).
    let inputs: Vec<In> = (0..cycles)
        .map(|cycle| {
            let pc = (cycle as u32) * 4;
            let pc_word = (pc / 4) as usize;
            let instr = if pc_word < program.len() {
                program[pc_word]
            } else {
                0 // NOP-equivalent illegal slot; CPU sets illegal flag
            };
            In {
                instr: bits::<32>(instr as u128),
                mem_rdata: bits::<32>(0),
            }
        })
        .collect();

    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    uut.run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| (s.input.1.instr, s.output))
        .collect()
}

#[test]
fn cpu_reset_has_pc_zero() {
    let trace = run_program(vec![addi(1, 0, 0)], 3);
    let first = trace.first().expect("at least one sample");
    assert_eq!(first.1.pc, bits::<32>(0));
    assert!(!first.1.illegal);
}

#[test]
fn cpu_pc_advances_by_4_after_addi() {
    // Program: addi x1, x0, 5; addi x2, x0, 7; addi x3, x0, 9
    let trace = run_program(
        vec![addi(1, 0, 5), addi(2, 0, 7), addi(3, 0, 9), addi(4, 0, 11)],
        5,
    );
    let pcs: Vec<u128> = trace.iter().map(|(_, o)| o.pc.raw()).collect();
    // PC should be 0, 4, 8, 12, 16 — observe the first few.
    assert!(
        pcs[0..4]
            == vec![0, 4, 8, 12]
                .into_iter()
                .map(|x| x as u128)
                .collect::<Vec<_>>(),
        "PC sequence: {pcs:?}",
    );
}

#[test]
fn cpu_addi_lands_in_register_file_observable_via_subsequent_arithmetic() {
    // Program:
    //   addi x1, x0, 5      ; x1 = 5
    //   addi x2, x0, 7      ; x2 = 7
    //   add  x3, x1, x2     ; x3 = 12
    //   sub  x4, x2, x1     ; x4 = 2
    //   addi x5, x3, 100    ; x5 = 112  (12 + 100)
    //   addi x6, x4, -1     ; x6 = 1   (2 + -1)
    //   add  x7, x5, x6     ; x7 = 113 (112 + 1)
    //
    // We verify by adding a final `addi x10, x7, 0` (mov-equivalent)
    // and observing that the ALU output exposed via `mem_addr`
    // (driven by the ALU result on the final cycle) equals 113.
    //
    // To observe the final value, we end with `sw x7, 0(x0)` so
    // the CPU drives `mem_addr = 0`, `mem_wdata = x7`, `mem_write = true`.
    let sw_x7_at_x0 = ((0u32 >> 5) & 0x7F) << 25  // imm[11:5] = 0
        | (7u32 & 0x1F) << 20          // rs2 = 7 (x7)
        | (0u32 & 0x1F) << 15          // rs1 = 0 (x0)
        | 2u32 << 12                   // funct3 = 010 (SW)
        | (0u32 & 0x1F) << 7           // imm[4:0] = 0
        | 0x23;                        // opcode = STORE
    let program = vec![
        addi(1, 0, 5),     // x1 = 5
        addi(2, 0, 7),     // x2 = 7
        add(3, 1, 2),      // x3 = 12
        sub(4, 2, 1),      // x4 = 2
        addi(5, 3, 100),   // x5 = 112
        addi(6, 4, -1),    // x6 = 1
        add(7, 5, 6),      // x7 = 113
        sw_x7_at_x0,       // observe x7 via mem_wdata
    ];
    let trace = run_program(program, 10);
    // Find the cycle where `mem_write` is asserted — that's the SW.
    let sw_cycle = trace
        .iter()
        .find(|(_, o)| o.mem_write)
        .expect("SW should fire at least once");
    assert_eq!(
        sw_cycle.1.mem_wdata,
        bits::<32>(113),
        "x7 should be 113 (= 5 + 7 + 100 + 1)",
    );
}

#[test]
fn cpu_lui_stores_upper_immediate() {
    // LUI x1, 0xABCDE  ; x1 = 0xABCDE000
    // SW  x1, 0(x0)    ; observe x1 via mem_wdata
    let sw_x1_at_x0 = ((0u32 >> 5) & 0x7F) << 25
        | (1u32 & 0x1F) << 20
        | (0u32 & 0x1F) << 15
        | 2u32 << 12
        | (0u32 & 0x1F) << 7
        | 0x23;
    let trace = run_program(vec![lui(1, 0xABCDE), sw_x1_at_x0], 5);
    let sw_cycle = trace
        .iter()
        .find(|(_, o)| o.mem_write)
        .expect("SW should fire");
    assert_eq!(sw_cycle.1.mem_wdata, bits::<32>(0xABCDE000));
}

#[test]
fn cpu_iverilog_round_trip() -> Result<(), RHDLError> {
    // Small program — same shape as the unit tests above but
    // shorter for faster simulation.
    let inputs: Vec<In> = (0..8)
        .map(|cycle| In {
            instr: bits::<32>(addi(1, 0, cycle as i32) as u128),
            mem_rdata: bits::<32>(0),
        })
        .collect();
    let uut = Cpu::default();
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
