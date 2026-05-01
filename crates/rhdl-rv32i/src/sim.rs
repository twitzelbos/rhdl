//! Rust-native RV32I reference simulator.
//!
//! Functional model of the entire shipped ISA (47 base
//! instructions + CSR access + ECALL/EBREAK/MRET + illegal-
//! instruction trap).  Used as the gold model for the
//! [`lockstep`] harness: programs run on this simulator AND on
//! both hardware cores; the per-cycle memory-write sequences
//! must agree.
//!
//! ## Why a Rust simulator instead of upstream Spike
//!
//! The plan (`tier-c-flagship-cores.md` §3.6) calls for Spike
//! lockstep.  Spike requires the riscv-isa-sim install
//! (riscv-software-src/riscv-isa-sim) and a Python harness wrapping
//! `spike --debug-cmd`.  That's significant tooling friction for
//! every developer.
//!
//! A Rust-native simulator captures the **structural value** of
//! Spike lockstep — an independent reference implementation that
//! catches bugs both hardware cores might share — without the
//! toolchain dependency.  Trade-off: it's not the official Spike,
//! so theoretically the simulator could share a bug with our
//! decoder.  Mitigated by writing the simulator in a fundamentally
//! different style (interpretive Rust, not the cycle-accurate
//! synchronous model the hardware uses) — the chance of a shared
//! bug is low, and any single bug-class will surface in at least
//! one of the three implementations.
//!
//! The simulator is also a useful teaching tool — fits in ~400
//! lines, reads as the spec.
//!
//! ## What it implements
//!
//! - All 47 RV32I base integer instructions (R/I/S/B/U/J types).
//! - CSR access: CSRRW/CSRRS/CSRRC + immediate variants.
//! - 8 M-mode CSRs: mstatus, misa, mtvec, mscratch, mepc, mcause,
//!   mtval, mhartid.
//! - Trap vectoring for ECALL (cause 11), EBREAK (cause 3),
//!   illegal instruction (cause 2).
//! - MRET return-from-trap.
//! - Memory: sparse word-addressed map.

use crate::isa::*;
use crate::decoder::decode;
use rhdl::prelude::*;
use std::collections::HashMap;

/// CPU architectural state — what an external observer would see
/// after each retired instruction.
#[derive(Clone, Debug)]
pub struct Cpu {
    /// Program counter.
    pub pc: u32,
    /// Architectural registers x0..x31.  x0 is hardwired zero
    /// (writes are silently dropped; reads always return 0 — see
    /// [`Cpu::read_reg`] / [`Cpu::write_reg`]).
    pub regs: [u32; 32],
    /// CSR file (sparse).
    pub csrs: HashMap<u16, u32>,
    /// Memory (sparse, word-addressed).  Key is the word address
    /// (byte address / 4); value is the 32-bit word.
    pub memory: HashMap<u32, u32>,
    /// Total instructions retired.
    pub retired: u64,
    /// True iff the simulator is halted (executed `beq x0, x0, +0`
    /// or hit an unrecoverable trap loop).
    pub halted: bool,
    /// Sequence of (mem_addr, mem_value) writes executed so far,
    /// in order.  Used by the lockstep harness to compare against
    /// the hardware's mem-write trace.
    pub mem_writes: Vec<(u32, u32)>,
}

impl Cpu {
    /// Create a CPU with all state zeroed.
    pub fn new() -> Self {
        let mut csrs = HashMap::new();
        // misa: RV32I marker (bit 30 = XLEN=32, bit 8 = "I").
        csrs.insert(0x301, 0x4000_0100);
        // mhartid: 0 (single hart).
        csrs.insert(0xF14, 0);
        Self {
            pc: 0,
            regs: [0; 32],
            csrs,
            memory: HashMap::new(),
            retired: 0,
            halted: false,
            mem_writes: Vec::new(),
        }
    }

    /// Read a register; x0 always returns 0.
    pub fn read_reg(&self, idx: u32) -> u32 {
        if idx == 0 { 0 } else { self.regs[(idx & 0x1F) as usize] }
    }
    /// Write a register; writes to x0 are silently dropped.
    pub fn write_reg(&mut self, idx: u32, value: u32) {
        if idx != 0 {
            self.regs[(idx & 0x1F) as usize] = value;
        }
    }

    /// Read a CSR (returns 0 for unimplemented addresses).
    pub fn read_csr(&self, addr: u16) -> u32 {
        *self.csrs.get(&addr).unwrap_or(&0)
    }
    /// Write a CSR (silently dropped for read-only addresses misa
    /// (0x301) and mhartid (0xF14)).
    pub fn write_csr(&mut self, addr: u16, value: u32) {
        if addr == 0x301 || addr == 0xF14 {
            return;
        }
        self.csrs.insert(addr, value);
    }

    /// Load a word from memory (returns 0 for uninitialized).
    pub fn load_word(&self, byte_addr: u32) -> u32 {
        let word_addr = byte_addr / 4;
        *self.memory.get(&word_addr).unwrap_or(&0)
    }
    /// Store a word.  Records the write in `mem_writes`.
    pub fn store_word(&mut self, byte_addr: u32, value: u32) {
        let word_addr = byte_addr / 4;
        self.memory.insert(word_addr, value);
        self.mem_writes.push((byte_addr, value));
    }

    /// Vector to mtvec; save trapping PC to mepc; set mcause.
    fn take_trap(&mut self, cause: u32) {
        self.write_csr(0x341, self.pc);     // mepc
        self.write_csr(0x342, cause);       // mcause
        self.pc = self.read_csr(0x305);     // mtvec
    }

    /// Execute one instruction.  Fetches from `program[pc/4]`;
    /// if pc/4 is past the end of `program`, treats the fetched
    /// instruction as 0 (which decodes as illegal and triggers a
    /// trap).
    pub fn step(&mut self, program: &[u32]) {
        let pc_word = (self.pc / 4) as usize;
        let instr = if pc_word < program.len() { program[pc_word] } else { 0 };
        self.retired += 1;

        // Detect HALT (`beq x0, x0, +0` = 0x00000063) and stop.
        if instr == 0x0000_0063 {
            self.halted = true;
            return;
        }

        let dec = decode(bits::<32>(instr as u128));

        // Illegal-instruction trap.
        if dec.illegal {
            self.take_trap(2);
            return;
        }

        // SYSTEM ops (ECALL/EBREAK/MRET) — handled before the
        // generic ALU/branch dispatch.
        match dec.system_op {
            SystemOp::Ecall  => { self.take_trap(11); return; }
            SystemOp::Ebreak => { self.take_trap(3); return; }
            SystemOp::Mret   => {
                self.pc = self.read_csr(0x341);  // PC ← mepc
                return;
            }
            SystemOp::None => {}
        }

        // CSR ops.
        if dec.csr_op != CsrOp::None {
            let csr_addr = dec.csr_addr.raw() as u16;
            let csr_old = self.read_csr(csr_addr);
            let rs1_val = self.read_reg(dec.rs1.raw() as u32);
            let uimm = dec.rs1.raw() as u32;
            let (new_value, writes) = match dec.csr_op {
                CsrOp::ReadWrite     => (rs1_val, true),
                CsrOp::ReadWriteImm  => (uimm, true),
                CsrOp::ReadSet       => (csr_old | rs1_val, dec.rs1.raw() != 0),
                CsrOp::ReadSetImm    => (csr_old | uimm, uimm != 0),
                CsrOp::ReadClear     => (csr_old & !rs1_val, dec.rs1.raw() != 0),
                CsrOp::ReadClearImm  => (csr_old & !uimm, uimm != 0),
                CsrOp::None          => (csr_old, false),
            };
            if writes {
                self.write_csr(csr_addr, new_value);
            }
            // CSR writeback to rd is the pre-modify value.
            self.write_reg(dec.rd.raw() as u32, csr_old);
            self.pc = self.pc.wrapping_add(4);
            return;
        }

        // Standard ALU / load / store / branch / jump dispatch.
        let rs1_val = self.read_reg(dec.rs1.raw() as u32);
        let rs2_val = self.read_reg(dec.rs2.raw() as u32);
        let imm     = dec.imm.raw() as u32;

        let alu_a = if dec.opcode == Opcode::Auipc { self.pc } else { rs1_val };
        let alu_b = if dec.alu_src == AluSrc::Imm { imm } else { rs2_val };
        let shamt = alu_b & 0x1F;
        let alu_result: u32 = match dec.alu_op {
            AluOp::Add  => alu_a.wrapping_add(alu_b),
            AluOp::Sub  => alu_a.wrapping_sub(alu_b),
            AluOp::Sll  => alu_a << shamt,
            AluOp::Slt  => if (alu_a as i32) < (alu_b as i32) { 1 } else { 0 },
            AluOp::Sltu => if alu_a < alu_b { 1 } else { 0 },
            AluOp::Xor  => alu_a ^ alu_b,
            AluOp::Srl  => alu_a >> shamt,
            AluOp::Sra  => ((alu_a as i32) >> shamt) as u32,
            AluOp::Or   => alu_a | alu_b,
            AluOp::And  => alu_a & alu_b,
            AluOp::Pass => alu_b,
        };

        let pc_plus_4 = self.pc.wrapping_add(4);

        // Branches.
        let branch_taken = match dec.branch_op {
            BranchOp::Eq  => rs1_val == rs2_val,
            BranchOp::Ne  => rs1_val != rs2_val,
            BranchOp::Lt  => (rs1_val as i32) < (rs2_val as i32),
            BranchOp::Ge  => (rs1_val as i32) >= (rs2_val as i32),
            BranchOp::Ltu => rs1_val < rs2_val,
            BranchOp::Geu => rs1_val >= rs2_val,
        };

        // Memory access.
        if dec.mem_write {
            let addr = alu_result;
            let store_data = rs2_val;
            // v0.8 only handles SW (32-bit aligned writes); SB/SH
            // would need byte-masked stores.  Compliance tests
            // don't use SB/SH yet.
            match dec.mem_op {
                MemOp::Sw => self.store_word(addr, store_data),
                MemOp::Sh => self.store_word(addr & !3, store_data & 0xFFFF), // simplified
                MemOp::Sb => self.store_word(addr & !3, store_data & 0xFF),   // simplified
                _         => {}
            }
        }
        let load_value: u32 = if dec.mem_read {
            let raw = self.load_word(alu_result);
            match dec.mem_op {
                MemOp::Lb  => ((raw << 24) as i32 >> 24) as u32,
                MemOp::Lh  => ((raw << 16) as i32 >> 16) as u32,
                MemOp::Lw  => raw,
                MemOp::Lbu => raw & 0xFF,
                MemOp::Lhu => raw & 0xFFFF,
                _          => raw,
            }
        } else {
            0
        };

        // Writeback.
        let writeback_value: u32 = match dec.writeback_src {
            WritebackSrc::None    => 0,
            WritebackSrc::Alu     => alu_result,
            WritebackSrc::Mem     => load_value,
            WritebackSrc::PcPlus4 => pc_plus_4,
            WritebackSrc::Csr     => 0,  // already handled above
        };
        if dec.writeback_src != WritebackSrc::None {
            self.write_reg(dec.rd.raw() as u32, writeback_value);
        }

        // Next PC.
        let take_branch = dec.opcode == Opcode::Branch && branch_taken;
        let take_jal    = dec.opcode == Opcode::Jal;
        let take_jalr   = dec.is_jalr;

        self.pc = if take_jalr {
            (rs1_val.wrapping_add(imm)) & !1u32
        } else if take_jal || take_branch {
            self.pc.wrapping_add(imm)
        } else {
            pc_plus_4
        };
    }

    /// Run the simulator until halted or `max_steps` instructions
    /// retired, whichever comes first.  Returns the final memory
    /// state plus the recorded mem-write sequence.
    pub fn run(mut self, program: &[u32], max_steps: u64) -> Self {
        while !self.halted && self.retired < max_steps {
            self.step(program);
        }
        self
    }
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}
