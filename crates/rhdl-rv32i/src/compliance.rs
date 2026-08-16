//! Hand-translated subset of the RISC-V `riscv-tests` rv32ui-p-*
//! suite, plus the framework that runs them.
//!
//! ## Why this isn't the upstream `riscv-tests`
//!
//! The plan (`tier-c-flagship-cores.md` §3.6) calls for running
//! the upstream `riscv-tests` repo against the core via an ELF
//! loader.  That requires the riscv-gnu-toolchain (a 30-60 minute
//! compile + several hundred MB of build artefacts) or vendoring
//! pre-built ELFs into the repo.  Both paths add significant
//! tooling friction; neither belongs in v0.5.
//!
//! Instead, this module **hand-translates the upstream tests' edge
//! cases into Rust functions** that emit equivalent instruction
//! sequences using our own encoding helpers.  Each test:
//!
//! 1. Encodes a series of `(id, expected, op-args…)` triples per
//!    the upstream test's edge-case table.
//! 2. Compiles to an instruction stream that does the operation
//!    on each triple, compares the result to `expected`, and
//!    branches to a "fail" handler with the test ID on mismatch.
//! 3. Falls through to a "pass" handler that writes 1 to a
//!    well-known scratchpad address (the test signature).
//! 4. The harness reads the signature: 1 = all pass; anything
//!    else = the failed sub-test ID.
//!
//! ## Migration path to upstream tests
//!
//! v0.6+ can swap in the real upstream tests by:
//! 1. Vendoring pre-built `rv32ui-p-*.bin` files (or `.hex`)
//!    under `crates/rhdl-rv32i/tests/vendor/riscv-tests/`.
//! 2. Adding a small ELF/HEX loader (`elf` crate or hand-rolled).
//! 3. Reading the vendored test's `.tohost` section address +
//!    final value as the success-signature contract.
//!
//! The framework here is structured so the test signature is a
//! word at scratchpad address 0 — same shape the upstream
//! `.tohost` mechanism uses.  Slot in the upstream binaries and
//! the existing assertion code keeps working.

use crate::cpu::{Cpu, In as SInIn, Out as SOut};
use crate::pipelined::{In as PIn, Out as POut, PipelinedCpu};
use rhdl::core::sim::ResetOrData;
use rhdl::prelude::*;

// ---- Encoding helpers (small, focused subset) --------------------

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
fn b_type(imm: i32, rs2: u32, rs1: u32, funct3: u32) -> u32 {
    let imm_u = (imm as u32) & 0x1FFF;
    let bit12 = (imm_u >> 12) & 1;
    let bit11 = (imm_u >> 11) & 1;
    let bits_10_5 = (imm_u >> 5) & 0x3F;
    let bits_4_1 = (imm_u >> 1) & 0xF;
    (bit12 << 31)
        | bits_10_5 << 25
        | (rs2 & 0x1F) << 20
        | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12
        | bits_4_1 << 8
        | bit11 << 7
        | 0x63
}
fn u_type(imm: u32, rd: u32, opcode: u32) -> u32 {
    (imm & 0xFFFF_F000) | (rd & 0x1F) << 7 | (opcode & 0x7F)
}

fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    i_type(imm, rs1, 0, rd, 0x13)
}
fn lui(rd: u32, imm20: u32) -> u32 {
    u_type(imm20 << 12, rd, 0x37)
}
fn add(rd: u32, rs1: u32, rs2: u32) -> u32 {
    r_type(0, rs2, rs1, 0, rd, 0x33)
}
fn sub(rd: u32, rs1: u32, rs2: u32) -> u32 {
    r_type(0x20, rs2, rs1, 0, rd, 0x33)
}
fn and(rd: u32, rs1: u32, rs2: u32) -> u32 {
    r_type(0, rs2, rs1, 7, rd, 0x33)
}
fn or(rd: u32, rs1: u32, rs2: u32) -> u32 {
    r_type(0, rs2, rs1, 6, rd, 0x33)
}
fn xor(rd: u32, rs1: u32, rs2: u32) -> u32 {
    r_type(0, rs2, rs1, 4, rd, 0x33)
}
fn sw(rs2: u32, rs1: u32, imm: i32) -> u32 {
    s_type(imm, rs2, rs1, 2, 0x23)
}
fn bne(rs1: u32, rs2: u32, imm: i32) -> u32 {
    b_type(imm, rs2, rs1, 1)
}
fn beq(rs1: u32, rs2: u32, imm: i32) -> u32 {
    b_type(imm, rs2, rs1, 0)
}

/// Load an arbitrary 32-bit constant into rd via LUI + ADDI.
///
/// The standard idiom: LUI gets the upper 20 bits; ADDI gets the
/// low 12 (sign-extended).  Because ADDI's immediate is sign-
/// extended, if the low 12 bits' top bit is 1 we need to add 1
/// to the upper 20 bits to compensate.
fn li(rd: u32, value: u32) -> [u32; 2] {
    let low = value & 0xFFF;
    // Sign-extend the low 12 bits.  ADDI will produce this value
    // (as a u32) for the low 12 bits.
    let low_se: u32 = if low & 0x800 != 0 {
        low | 0xFFFF_F000
    } else {
        low
    };
    // Wrapping subtraction handles the sign-extension carry.
    let high: u32 = (value.wrapping_sub(low_se) >> 12) & 0xFFFFF;
    let low_signed: i32 = if low & 0x800 != 0 {
        // Treat as negative i32 in -2048..0 range.
        (low as i32) | !0xFFF
    } else {
        low as i32
    };
    [lui(rd, high), addi(rd, rd, low_signed)]
}

// ---- Compliance-test framework -----------------------------------

/// One sub-test in a compliance program.  Modelled after the
/// `TEST_RR_OP(id, op, expected, a, b)` macro from upstream
/// `riscv-tests`.
#[derive(Clone, Copy, Debug)]
pub struct RrTest {
    /// The sub-test number used as the failure code.  Numbering
    /// follows the upstream tests' convention (the first sub-test
    /// is usually 2, since 1 is reserved for "no failure").
    pub id: u32,
    /// The expected `op(a, b)` result.
    pub expected: u32,
    /// First operand.
    pub a: u32,
    /// Second operand.
    pub b: u32,
}

/// Encode an `op` to apply to a list of `RrTest`s.  Used by the
/// per-instruction test programs (`make_add_program`, etc.).
type RrEncoder = fn(rd: u32, rs1: u32, rs2: u32) -> u32;

/// Build a program that runs each `RrTest` through `op` and
/// branches to a fail handler with the test ID on mismatch.
///
/// Register allocation:
/// - x10..x12: scratch (a, b, result)
/// - x13:     expected
/// - x28:     test signature destination (set up at start)
/// - x29:     final-success constant 1
/// - x30:     scratchpad-base address constant
///
/// The test signature lives at scratchpad word 0.  Pass writes 1;
/// fail writes the failing test's ID.
pub fn make_rr_program(op: RrEncoder, tests: &[RrTest]) -> Vec<u32> {
    let mut prog: Vec<u32> = Vec::new();
    // Setup: x30 = 0 (data-mem base), x29 = 1 (pass code).
    prog.push(addi(30, 0, 0)); // x30 = 0
    prog.push(addi(29, 0, 1)); // x29 = 1

    // Per-test sequence.  Each sub-test:
    //   li  x10, a
    //   li  x11, b
    //   op  x12, x10, x11
    //   li  x13, expected
    //   bne x12, x13, +<offset to fail handler>
    //
    // The fail-jump offset is patched up after the full program
    // is laid out (we know the fail handler's PC then).  For now
    // we emit a placeholder relative offset and patch at the end.
    let mut fail_patch_sites: Vec<(usize, u32)> = Vec::new();
    for t in tests {
        // li x10, a
        prog.extend_from_slice(&li(10, t.a));
        // li x11, b
        prog.extend_from_slice(&li(11, t.b));
        // op x12, x10, x11
        prog.push(op(12, 10, 11));
        // li x13, expected
        prog.extend_from_slice(&li(13, t.expected));
        // bne x12, x13, fail_handler  (placeholder offset; patch later)
        fail_patch_sites.push((prog.len(), t.id));
        prog.push(bne(12, 13, 0)); // offset 0, patched
    }

    // Pass handler: write x29 (= 1) to mem[0] then loop forever.
    let pass_handler_pc = (prog.len() * 4) as u32;
    prog.push(sw(29, 30, 0)); // mem[0] = 1
    prog.push(beq(0, 0, 0)); // beq x0, x0, +0  (infinite loop)

    // Fail handler: write the fail code (loaded into x14 by the
    // bne-target setup below) to mem[0], then loop forever.
    //
    // For simplicity we have ONE fail handler per test ID — emit
    // a small fail block per sub-test that loads its own ID into
    // x14, writes it, and loops.  Less compact but trivially
    // patchable.
    for (site, id) in &fail_patch_sites {
        let fail_pc = (prog.len() * 4) as u32;
        // Patch the bne at `site` to jump to `fail_pc`.
        let bne_pc = (*site * 4) as u32;
        let offset = fail_pc as i32 - bne_pc as i32;
        prog[*site] = bne(12, 13, offset);
        // Emit the fail block.
        prog.extend_from_slice(&li(14, *id));
        prog.push(sw(14, 30, 0)); // mem[0] = id
        prog.push(beq(0, 0, 0)); // infinite loop
    }

    let _ = pass_handler_pc;
    prog
}

/// Run a compliance program through a CPU and return the
/// scratchpad word at offset 0 (the test signature).  Returns 1
/// on pass, the failed test's ID on fail, or 0 if the program
/// didn't reach either handler.
pub fn run_signature_single(program: Vec<u32>, max_cycles: usize) -> u32 {
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
            let instr: u32 = if pc_word < program.len() {
                program[pc_word]
            } else {
                0
            };
            let read_word = (out.mem_addr.raw() / 4) as usize;
            let mem_rdata: u32 = if read_word < data_mem.len() {
                data_mem[read_word]
            } else {
                0
            };
            Some(ResetOrData::Data(SInIn {
                instr: bits::<32>(instr as u128),
                mem_rdata: bits::<32>(mem_rdata as u128),
                int_pending: bits::<32>(0),
            }))
        },
        100,
    )
    .for_each(drop);
    data_mem[0]
}

/// Same as [`run_signature_single`] but for the pipelined CPU.
pub fn run_signature_pipelined(program: Vec<u32>, max_cycles: usize) -> u32 {
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
            total_cycles += 1;
            if out.mem_write {
                let addr_word = (out.mem_addr.raw() / 4) as usize;
                if addr_word < data_mem.len() {
                    data_mem[addr_word] = out.mem_wdata.raw() as u32;
                }
            }
            let pc_word = (out.pc.raw() / 4) as usize;
            let instr: u32 = if pc_word < program.len() {
                program[pc_word]
            } else {
                0
            };
            let read_word = (out.mem_addr.raw() / 4) as usize;
            let mem_rdata: u32 = if read_word < data_mem.len() {
                data_mem[read_word]
            } else {
                0
            };
            Some(ResetOrData::Data(PIn {
                instr: bits::<32>(instr as u128),
                mem_rdata: bits::<32>(mem_rdata as u128),
                int_pending: bits::<32>(0),
            }))
        },
        100,
    )
    .for_each(drop);
    data_mem[0]
}

// ---- Hand-translated test programs -------------------------------

/// `rv32ui-p-add` edge cases.  Source: riscv-software-src/riscv-tests
/// isa/rv32ui/add.S.  Selected representative cases — full upstream
/// has ~40 sub-tests; these cover the main signed/unsigned/overflow
/// patterns.
pub fn make_add_program() -> Vec<u32> {
    let tests = vec![
        RrTest {
            id: 2,
            expected: 0x00000000,
            a: 0x00000000,
            b: 0x00000000,
        },
        RrTest {
            id: 3,
            expected: 0x00000002,
            a: 0x00000001,
            b: 0x00000001,
        },
        RrTest {
            id: 4,
            expected: 0x0000000a,
            a: 0x00000003,
            b: 0x00000007,
        },
        RrTest {
            id: 5,
            expected: 0xffff8000,
            a: 0x00000000,
            b: 0xffff8000,
        },
        RrTest {
            id: 6,
            expected: 0x80000000,
            a: 0x80000000,
            b: 0x00000000,
        },
        RrTest {
            id: 7,
            expected: 0x7fff8000,
            a: 0x80000000,
            b: 0xffff8000,
        },
        RrTest {
            id: 8,
            expected: 0x00007fff,
            a: 0x00000000,
            b: 0x00007fff,
        },
        RrTest {
            id: 9,
            expected: 0x7fffffff,
            a: 0x7fffffff,
            b: 0x00000000,
        },
        RrTest {
            id: 10,
            expected: 0x80007ffe,
            a: 0x7fffffff,
            b: 0x00007fff,
        },
        RrTest {
            id: 11,
            expected: 0x80007fff,
            a: 0x80000000,
            b: 0x00007fff,
        },
        RrTest {
            id: 12,
            expected: 0x7fff7fff,
            a: 0x7fffffff,
            b: 0xffff8000,
        },
        RrTest {
            id: 13,
            expected: 0xffffffff,
            a: 0x00000000,
            b: 0xffffffff,
        },
        RrTest {
            id: 14,
            expected: 0x00000000,
            a: 0xffffffff,
            b: 0x00000001,
        },
        RrTest {
            id: 15,
            expected: 0xfffffffe,
            a: 0xffffffff,
            b: 0xffffffff,
        },
        RrTest {
            id: 16,
            expected: 0x80000000,
            a: 0x00000001,
            b: 0x7fffffff,
        },
    ];
    make_rr_program(add, &tests)
}

/// `rv32ui-p-sub` edge cases.
pub fn make_sub_program() -> Vec<u32> {
    let tests = vec![
        RrTest {
            id: 2,
            expected: 0x00000000,
            a: 0x00000000,
            b: 0x00000000,
        },
        RrTest {
            id: 3,
            expected: 0x00000000,
            a: 0x00000001,
            b: 0x00000001,
        },
        RrTest {
            id: 4,
            expected: 0xfffffffc,
            a: 0x00000003,
            b: 0x00000007,
        },
        RrTest {
            id: 5,
            expected: 0x00008000,
            a: 0x00000000,
            b: 0xffff8000,
        },
        RrTest {
            id: 6,
            expected: 0x80000000,
            a: 0x80000000,
            b: 0x00000000,
        },
        RrTest {
            id: 7,
            expected: 0x80008000,
            a: 0x80000000,
            b: 0xffff8000,
        },
        RrTest {
            id: 8,
            expected: 0xffff8001,
            a: 0x00000000,
            b: 0x00007fff,
        },
        RrTest {
            id: 9,
            expected: 0x7fffffff,
            a: 0x7fffffff,
            b: 0x00000000,
        },
        RrTest {
            id: 10,
            expected: 0x7fff8000,
            a: 0x7fffffff,
            b: 0x00007fff,
        },
    ];
    make_rr_program(sub, &tests)
}

/// `rv32ui-p-and` edge cases.  Each `expected` is the actual
/// `a & b` — I made the mistake of hand-computing them once and
/// got several wrong; regenerated by formula below.
pub fn make_and_program() -> Vec<u32> {
    // Pairs of (id, a, b); expected is computed.
    let pairs: Vec<(u32, u32, u32)> = vec![
        (2, 0xff00ff00, 0xf0f0f0f0),
        (3, 0x0ff00ff0, 0xf00ff00f),
        (4, 0x00ff00ff, 0x0f0f0f0f),
        (5, 0xf00ff00f, 0x0ff00ff0),
        (6, 0xffffffff, 0x00000000),
        (7, 0xffffffff, 0xffffffff),
    ];
    let tests: Vec<RrTest> = pairs
        .into_iter()
        .map(|(id, a, b)| RrTest {
            id,
            expected: a & b,
            a,
            b,
        })
        .collect();
    make_rr_program(and, &tests)
}

/// `rv32ui-p-or` edge cases.
pub fn make_or_program() -> Vec<u32> {
    let pairs: Vec<(u32, u32, u32)> = vec![
        (2, 0xff00ff00, 0x0ff00ff0),
        (3, 0x0ff00ff0, 0xf00ff00f),
        (4, 0xffffffff, 0x00000000),
        (5, 0xffffffff, 0xffffffff),
        (6, 0x00000000, 0x00000000),
    ];
    let tests: Vec<RrTest> = pairs
        .into_iter()
        .map(|(id, a, b)| RrTest {
            id,
            expected: a | b,
            a,
            b,
        })
        .collect();
    make_rr_program(or, &tests)
}

/// `rv32ui-p-xor` edge cases.
pub fn make_xor_program() -> Vec<u32> {
    let pairs: Vec<(u32, u32, u32)> = vec![
        (2, 0xff00ff00, 0x0f0f0f0f),
        (3, 0xff00ff00, 0x00000000),
        (4, 0xff00ff00, 0xff00ff00),
        (5, 0xff00ff00, 0x00ff00ff),
        (6, 0xffffffff, 0xffffffff),
    ];
    let tests: Vec<RrTest> = pairs
        .into_iter()
        .map(|(id, a, b)| RrTest {
            id,
            expected: a ^ b,
            a,
            b,
        })
        .collect();
    make_rr_program(xor, &tests)
}

/// `rv32ui-p-addi` — I-type ADDI edge cases.
///
/// ADDI's second operand is a 12-bit signed immediate (range
/// -2048..2047), not a register.  We use a slightly different
/// program structure: the test cases hold (id, expected, a,
/// imm12) with imm12 fitting in the spec's range.
pub fn make_addi_program() -> Vec<u32> {
    // Per-test: li x10, a; addi x12, x10, imm; li x13, expected; bne ...
    let tests = vec![
        (2u32, 0x00000000u32, 0x00000000u32, 0x000i32),
        (3, 0x00000002, 0x00000001, 0x001),
        (4, 0x0000000a, 0x00000003, 0x007),
        (5, 0xfffff800, 0x00000000, -2048), // imm = 0x800 (sign-extended to 0xFFFF_F800)
        (6, 0x80000000, 0x80000000, 0x000),
        (7, 0x7ffff800, 0x80000000, -2048),
        (8, 0x000007ff, 0x00000000, 0x7FF), // imm = +2047
        (9, 0x7fffffff, 0x7fffffff, 0x000),
        (10, 0x800007fe, 0x7fffffff, 0x7FF),
        (11, 0x800007ff, 0x80000000, 0x7FF),
        (12, 0x7ffff7ff, 0x7fffffff, -2049 + 1), // imm = -2048
        (13, 0xffffffff, 0x00000000, -1),
        (14, 0x00000000, 0xffffffff, 0x001),
        (15, 0xfffffffe, 0xffffffff, -1),
        (16, 0x80000000, 0x7fffffff, 0x001),
    ];

    let mut prog: Vec<u32> = Vec::new();
    prog.push(addi(30, 0, 0));
    prog.push(addi(29, 0, 1));

    let mut fail_patch_sites: Vec<(usize, u32)> = Vec::new();
    for (id, expected, a, imm12) in tests {
        prog.extend_from_slice(&li(10, a));
        prog.push(addi(12, 10, imm12));
        prog.extend_from_slice(&li(13, expected));
        fail_patch_sites.push((prog.len(), id));
        prog.push(bne(12, 13, 0));
    }
    prog.push(sw(29, 30, 0));
    prog.push(beq(0, 0, 0));
    for (site, id) in &fail_patch_sites {
        let fail_pc = (prog.len() * 4) as u32;
        let bne_pc = (*site * 4) as u32;
        let offset = fail_pc as i32 - bne_pc as i32;
        prog[*site] = bne(12, 13, offset);
        prog.extend_from_slice(&li(14, *id));
        prog.push(sw(14, 30, 0));
        prog.push(beq(0, 0, 0));
    }
    prog
}
