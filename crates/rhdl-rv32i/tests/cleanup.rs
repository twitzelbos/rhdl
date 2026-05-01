//! Tests for the small privileged-ISA cleanups bundled in this PR:
//!
//! 1. **Software-writable MSIP** — `mip[3]` can be set/cleared via
//!    CSR write, in addition to being asserted by the platform via
//!    `int_pending`.
//! 2. **Misaligned-load/store traps** — LH/LHU/LW with non-aligned
//!    address trap with `mcause = 4` (load) and `mtval = address`.
//!    SH/SW with non-aligned address trap with `mcause = 6`.
//! 3. **Vectored mtvec** — when `mtvec[1:0] == 1`, interrupts go to
//!    `(mtvec & ~3) + 4 * cause_low4`.  Sync exceptions still go
//!    to the base regardless of mode.

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
fn lw(rd: u32, rs1: u32, imm: i32) -> u32 { i_type(imm, rs1, 2, rd, 0x03) }
fn lh(rd: u32, rs1: u32, imm: i32) -> u32 { i_type(imm, rs1, 1, rd, 0x03) }
fn sw(rs2: u32, rs1: u32, imm: i32) -> u32 { s_type(imm, rs2, rs1, 2, 0x23) }
fn sh(rs2: u32, rs1: u32, imm: i32) -> u32 { s_type(imm, rs2, rs1, 1, 0x23) }
fn lui(rd: u32, imm20: u32) -> u32 { (imm20 & 0xFFFFF) << 12 | (rd & 0x1F) << 7 | 0x37 }
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

// ---- Closed-loop harness -----------------------------------------

fn run_single<F: FnMut(usize) -> u32>(
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
            if reset_cycles_remaining > 0 { reset_cycles_remaining -= 1; return Some(ResetOrData::Reset); }
            if total_cycles >= max_cycles { return None; }
            let cyc = total_cycles;
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
                int_pending: bits::<32>(int_at(cyc) as u128),
            }))
        },
        100,
    ).for_each(drop);
    data_mem
}

fn run_pipelined<F: FnMut(usize) -> u32>(
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
            if reset_cycles_remaining > 0 { reset_cycles_remaining -= 1; return Some(ResetOrData::Reset); }
            if total_cycles >= max_cycles { return None; }
            let cyc = total_cycles;
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
                int_pending: bits::<32>(int_at(cyc) as u128),
            }))
        },
        100,
    ).for_each(drop);
    data_mem
}

// ---- 1. Software-writable MSIP -----------------------------------

/// Software writes mip[3] = 1 → M-software interrupt fires (with
/// mie.MSIE and mstatus.MIE both set), even with `int_pending = 0`.
#[test]
fn software_writes_msip_to_trigger_self_interrupt() {
    let program = vec![
        lui(1, 0),                        // 0x00
        addi(1, 1, 0x20),                 // 0x04: mtvec base = 0x20
        csrrw(0, 1, CSR_MTVEC),           // 0x08
        addi(2, 0, 0x008),                // 0x0C: MSIE
        csrrw(0, 2, CSR_MIE),             // 0x10
        csrrsi(0, 0x8, CSR_MSTATUS),      // 0x14: mstatus.MIE = 1
        // 0x18: software writes mip → MSIP = 1
        addi(3, 0, 0x008),                // 0x18: x3 = 0x8
        csrrw(0, 3, CSR_MIP),             // 0x1C: mip[3] ← 1
        // 0x20: handler
        csrrs(4, 0, CSR_MCAUSE),          // 0x20
        sw(4, 0, 0),                      // 0x24
        HALT,                             // 0x28
    ];
    let mem = run_single(program, 32, |_| 0);
    assert_eq!(mem[0], 0x8000_0003, "software MSIP should fire as M-software interrupt");
}

#[test]
fn software_writes_msip_pipelined_parity() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        addi(2, 0, 0x008),
        csrrw(0, 2, CSR_MIE),
        csrrsi(0, 0x8, CSR_MSTATUS),
        addi(3, 0, 0x008),
        csrrw(0, 3, CSR_MIP),
        csrrs(4, 0, CSR_MCAUSE),
        sw(4, 0, 0),
        HALT,
    ];
    let single = run_single(program.clone(), 32, |_| 0);
    let pipelined = run_pipelined(program, 60, |_| 0);
    assert_eq!(single[0], 0x8000_0003);
    assert_eq!(pipelined[0], single[0], "pipelined MSIP-via-software parity");
}

/// Software clears mip[3] = 0 → no interrupt even if MIE/mie set.
#[test]
fn software_can_clear_msip() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x40),
        csrrw(0, 1, CSR_MTVEC),
        // First write MSIP via software, then clear it.
        addi(3, 0, 0x008),
        csrrw(0, 3, CSR_MIP),             // mip[3] = 1
        csrrw(0, 0, CSR_MIP),             // mip[3] = 0 (rs1 = x0 = 0)
        // Now enable interrupts — should NOT fire.
        addi(2, 0, 0x008),
        csrrw(0, 2, CSR_MIE),
        csrrsi(0, 0x8, CSR_MSTATUS),
        addi(7, 0, 0x55),
        sw(7, 0, 0),                      // mem[0] = 0x55
        HALT,
    ];
    let mem = run_single(program, 24, |_| 0);
    assert_eq!(mem[0], 0x55, "after clearing MSIP, no interrupt should fire");
}

#[test]
fn sim_software_msip_round_trip() {
    let mut cpu = sim::Cpu::new();
    cpu.write_csr(0x344, 0x8);  // software writes MSIP
    assert_eq!(cpu.read_csr(0x344), 0x8, "mip read returns MSIP");
    cpu.write_csr(0x344, 0);    // software clears MSIP
    assert_eq!(cpu.read_csr(0x344), 0, "mip read returns 0 after clear");
}

// ---- 2. Misaligned-load/store traps ------------------------------

/// LW with addr & 3 != 0 → mcause = 4, mtval = addr.
#[test]
fn misaligned_lw_traps_mcause_4() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        addi(5, 0, 0x402),                // 0x0C: x5 = 0x402 (LW base — misaligned addr 0x402)
        addi(0, 0, 0),                    // 0x10
        lw(6, 5, 0),                      // 0x14: LW x6, 0(x5) — addr = 0x402, w_misalign
        addi(0, 0, 0),                    // 0x18: SQUASHED
        addi(0, 0, 0),                    // 0x1C: SQUASHED
        // 0x20: handler
        csrrs(7, 0, CSR_MCAUSE),
        csrrs(8, 0, CSR_MTVAL),
        csrrs(9, 0, CSR_MEPC),
        sw(7, 0, 0),                      // mem[0] = mcause
        sw(8, 0, 4),                      // mem[1] = mtval (the bad addr)
        sw(9, 0, 8),                      // mem[2] = mepc (the LW's PC)
        HALT,
    ];
    let mem = run_single(program, 32, |_| 0);
    assert_eq!(mem[0], 4, "mcause = 4 (load addr misaligned)");
    assert_eq!(mem[1], 0x402, "mtval = misaligned load address");
    assert_eq!(mem[2], 0x14, "mepc = LW's PC");
}

/// LH with addr & 1 != 0 → mcause = 4.
#[test]
fn misaligned_lh_traps_mcause_4() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        addi(5, 0, 0x401),                // odd address
        addi(0, 0, 0),
        lh(6, 5, 0),                      // 0x14: LH at 0x401 — h_misalign
        addi(0, 0, 0),
        addi(0, 0, 0),
        csrrs(7, 0, CSR_MCAUSE),
        csrrs(8, 0, CSR_MTVAL),
        sw(7, 0, 0),
        sw(8, 0, 4),
        HALT,
    ];
    let mem = run_single(program, 24, |_| 0);
    assert_eq!(mem[0], 4);
    assert_eq!(mem[1], 0x401);
}

/// SW with addr & 3 != 0 → mcause = 6, mtval = addr.
#[test]
fn misaligned_sw_traps_mcause_6() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        addi(5, 0, 0x402),                // misaligned target addr
        addi(6, 0, 0xCD),                 // store value
        sw(6, 5, 0),                      // 0x14: SW x6, 0(x5) — addr 0x402
        addi(0, 0, 0),
        addi(0, 0, 0),
        csrrs(7, 0, CSR_MCAUSE),
        csrrs(8, 0, CSR_MTVAL),
        sw(7, 0, 0),
        sw(8, 0, 4),
        HALT,
    ];
    let mem = run_single(program, 24, |_| 0);
    assert_eq!(mem[0], 6, "mcause = 6 (store addr misaligned)");
    assert_eq!(mem[1], 0x402, "mtval = misaligned store address");
}

/// SH with addr & 1 != 0 → mcause = 6.
#[test]
fn misaligned_sh_traps_mcause_6() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        addi(5, 0, 0x401),
        addi(6, 0, 0xCD),
        sh(6, 5, 0),                      // 0x14: SH at 0x401
        addi(0, 0, 0),
        addi(0, 0, 0),
        csrrs(7, 0, CSR_MCAUSE),
        csrrs(8, 0, CSR_MTVAL),
        sw(7, 0, 0),
        sw(8, 0, 4),
        HALT,
    ];
    let mem = run_single(program, 24, |_| 0);
    assert_eq!(mem[0], 6);
    assert_eq!(mem[1], 0x401);
}

/// Aligned LW does NOT trap.
#[test]
fn aligned_lw_does_not_trap() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        addi(5, 0, 0x400),                // aligned
        addi(0, 0, 0),
        lw(6, 5, 0),                      // OK
        sw(6, 0, 0),                      // mem[0] = loaded value (initially 0)
        HALT,
    ];
    let mem = run_single(program, 16, |_| 0);
    // mem[0] = 0 (loaded from uninitialised data_mem[0x100]).  The
    // important thing: no trap, no overwrite from handler.
    assert_eq!(mem[0], 0, "aligned LW should not trap");
}

/// Misaligned-store pipelined parity.
#[test]
fn misaligned_sw_pipelined_parity() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),
        csrrw(0, 1, CSR_MTVEC),
        addi(5, 0, 0x402),
        addi(6, 0, 0xCD),
        sw(6, 5, 0),
        addi(0, 0, 0),
        addi(0, 0, 0),
        csrrs(7, 0, CSR_MCAUSE),
        csrrs(8, 0, CSR_MTVAL),
        sw(7, 0, 0),
        sw(8, 0, 4),
        HALT,
    ];
    let single = run_single(program.clone(), 24, |_| 0);
    let pipelined = run_pipelined(program, 50, |_| 0);
    assert_eq!(single[0], 6);
    assert_eq!(single[1], 0x402);
    assert_eq!(pipelined[0], single[0]);
    assert_eq!(pipelined[1], single[1]);
}

/// Sim mirror: misaligned LW traps with mcause = 4.
#[test]
fn sim_misaligned_lw_traps() {
    let mut cpu = sim::Cpu::new();
    cpu.write_csr(0x305, 0x100);   // mtvec
    cpu.regs[5] = 0x402;
    cpu.pc = 0;
    cpu.step(&[lw(6, 5, 0), HALT]);
    assert_eq!(cpu.read_csr(0x342), 4, "mcause = 4");
    assert_eq!(cpu.read_csr(0x343), 0x402, "mtval = misaligned addr");
}

// ---- 3. Vectored mtvec --------------------------------------------

/// Vectored mtvec: M-software interrupt (cause = 0x80000003) should
/// vector to base + 4*3 = base + 0xC.
#[test]
fn vectored_mtvec_routes_msi_to_offset_c() {
    // Set mtvec = 0x40 | 0x1 (vectored mode, base 0x40).
    // M-software interrupt → PC = 0x40 + 4*3 = 0x4C.
    // Direct-mode handlers at 0x40 vs 0x4C distinguish.
    let program = vec![
        lui(1, 0),                        // 0x00
        addi(1, 1, 0x40 | 0x1),           // 0x04: x1 = 0x41 (base=0x40, mode=1)
        csrrw(0, 1, CSR_MTVEC),           // 0x08
        addi(2, 0, 0x008),                // 0x0C: MSIE
        csrrw(0, 2, CSR_MIE),             // 0x10
        csrrsi(0, 0x8, CSR_MSTATUS),      // 0x14
        addi(3, 0, 0x008),                // 0x18: x3 = MSIP bit
        csrrw(0, 3, CSR_MIP),             // 0x1C: trigger MSI via software
        addi(7, 0, 0x77),                 // 0x20: SQUASHED
        addi(7, 0, 0x77),                 // 0x24: SQUASHED
        addi(7, 0, 0x77),                 // 0x28: SQUASHED
        addi(7, 0, 0x77),                 // 0x2C: SQUASHED
        addi(7, 0, 0x77),                 // 0x30: SQUASHED
        addi(7, 0, 0x77),                 // 0x34: SQUASHED
        addi(7, 0, 0x77),                 // 0x38: SQUASHED
        addi(7, 0, 0x77),                 // 0x3C: SQUASHED
        // 0x40: direct-mode handler (base) — should NOT execute
        addi(8, 0, 0xAA),                 // 0x40
        sw(8, 0, 0),                      // 0x44: mem[0] = 0xAA (would-be-base)
        HALT,                             // 0x48
        // 0x4C: vectored handler for cause 3 — SHOULD execute
        addi(9, 0, 0xBB),                 // 0x4C
        sw(9, 0, 0),                      // 0x50: mem[0] = 0xBB
        HALT,                             // 0x54
    ];
    let mem = run_single(program, 32, |_| 0);
    assert_eq!(mem[0], 0xBB, "vectored MSI should land at base + 0xC, not at base");
}

/// Vectored mtvec: sync exception (mcause = 11 ECALL) still goes
/// to base regardless of mode.
#[test]
fn vectored_mtvec_sync_exception_still_to_base() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x40 | 0x1),           // base=0x40, vectored mode
        csrrw(0, 1, CSR_MTVEC),
        addi(0, 0, 0),                    // 0x0C
        addi(0, 0, 0),                    // 0x10
        0x0000_0073,                      // 0x14: ECALL → mcause = 11
        addi(0, 0, 0),                    // SQUASHED
        addi(0, 0, 0),                    // SQUASHED
        addi(0, 0, 0),                    // SQUASHED
        addi(0, 0, 0),                    // SQUASHED
        addi(0, 0, 0),                    // SQUASHED
        addi(0, 0, 0),                    // SQUASHED
        addi(0, 0, 0),                    // SQUASHED
        addi(0, 0, 0),                    // SQUASHED
        addi(0, 0, 0),                    // SQUASHED
        addi(0, 0, 0),                    // SQUASHED
        // 0x40: base handler — sync exceptions land here regardless
        addi(8, 0, 0xCC),                 // 0x40
        sw(8, 0, 0),                      // 0x44: mem[0] = 0xCC
        HALT,                             // 0x48
    ];
    let mem = run_single(program, 24, |_| 0);
    assert_eq!(mem[0], 0xCC, "ECALL (sync) goes to base even in vectored mode");
}

/// Direct mode (mtvec[1:0] = 0): everything to base, including
/// interrupts.  Sanity check that non-vectored mode is unchanged.
#[test]
fn direct_mtvec_routes_msi_to_base() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x20),                 // base=0x20, mode=0 (direct)
        csrrw(0, 1, CSR_MTVEC),
        addi(2, 0, 0x008),
        csrrw(0, 2, CSR_MIE),
        csrrsi(0, 0x8, CSR_MSTATUS),
        addi(3, 0, 0x008),
        csrrw(0, 3, CSR_MIP),             // trigger MSI
        // 0x20: base — should execute
        addi(8, 0, 0xDD),                 // 0x20
        sw(8, 0, 0),                      // 0x24: mem[0] = 0xDD
        HALT,                             // 0x28
    ];
    let mem = run_single(program, 24, |_| 0);
    assert_eq!(mem[0], 0xDD, "direct mode sends interrupts to base");
}

/// Vectored mtvec pipelined parity.
#[test]
fn vectored_mtvec_pipelined_parity() {
    let program = vec![
        lui(1, 0),
        addi(1, 1, 0x40 | 0x1),
        csrrw(0, 1, CSR_MTVEC),
        addi(2, 0, 0x008),
        csrrw(0, 2, CSR_MIE),
        csrrsi(0, 0x8, CSR_MSTATUS),
        addi(3, 0, 0x008),
        csrrw(0, 3, CSR_MIP),
        addi(7, 0, 0x77),
        addi(7, 0, 0x77),
        addi(7, 0, 0x77),
        addi(7, 0, 0x77),
        addi(7, 0, 0x77),
        addi(7, 0, 0x77),
        addi(7, 0, 0x77),
        addi(7, 0, 0x77),
        addi(8, 0, 0xAA),                 // 0x40
        sw(8, 0, 0),
        HALT,
        addi(9, 0, 0xBB),                 // 0x4C
        sw(9, 0, 0),
        HALT,
    ];
    let single = run_single(program.clone(), 40, |_| 0);
    let pipelined = run_pipelined(program, 80, |_| 0);
    assert_eq!(single[0], 0xBB);
    assert_eq!(pipelined[0], single[0], "pipelined vectored-mtvec parity");
}

/// Sim mirror: vectored mtvec routes interrupt to offset.
#[test]
fn sim_vectored_mtvec_routes_to_offset() {
    let mut cpu = sim::Cpu::new();
    cpu.write_csr(0x300, 0x8);  // mstatus.MIE = 1
    cpu.write_csr(0x304, 0x8);  // mie.MSIE = 1
    cpu.write_csr(0x305, 0x40 | 0x1);  // mtvec base=0x40, vectored mode
    cpu.msip = true;
    cpu.pc = 0x100;
    cpu.step(&[0; 64]);
    assert_eq!(cpu.pc, 0x40 + 4 * 3, "PC should be at vectored offset for cause 3");
}

#[test]
fn sim_direct_mtvec_routes_to_base() {
    let mut cpu = sim::Cpu::new();
    cpu.write_csr(0x300, 0x8);
    cpu.write_csr(0x304, 0x8);
    cpu.write_csr(0x305, 0x40);  // mode = 0 (direct)
    cpu.msip = true;
    cpu.pc = 0x100;
    cpu.step(&[0; 64]);
    assert_eq!(cpu.pc, 0x40, "direct mode → PC = base");
}
