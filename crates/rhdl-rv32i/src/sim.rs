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
    /// External-interrupt-pending vector (mirrors into `mip`).
    /// Bits 7 (M-timer) and 11 (M-external) flow from this input
    /// directly into `mip`.  Bit 3 (M-software) is OR'ed with the
    /// software-writable MSIP register (see [`Cpu::msip`]).
    /// Set by the test harness; the simulator polls this at the
    /// start of each `step` (interrupts taken between instructions).
    pub int_pending: u32,
    /// Software-writable MSIP register (bit 3 of `mip` when this
    /// is set).  CSR writes to mip update this; reads of mip return
    /// `(int_pending & 0x880) | (msip ? 0x8 : 0)`.
    pub msip: bool,
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
            int_pending: 0,
            msip: false,
        }
    }

    /// Effective `mip` value: platform bits 3/7/11 from
    /// `int_pending` OR'd with software MSIP (`msip`).
    pub fn effective_mip(&self) -> u32 {
        (self.int_pending & 0x888) | if self.msip { 0x8 } else { 0 }
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
    /// `mip` (0x344) is composed from `int_pending` and `msip`.
    pub fn read_csr(&self, addr: u16) -> u32 {
        if addr == 0x344 {
            return self.effective_mip();
        }
        *self.csrs.get(&addr).unwrap_or(&0)
    }
    /// Write a CSR (silently dropped for read-only addresses misa
    /// (0x301) and mhartid (0xF14); `mip` write only updates the
    /// software-writable MSIP bit).
    pub fn write_csr(&mut self, addr: u16, value: u32) {
        if addr == 0x301 || addr == 0xF14 {
            return;
        }
        if addr == 0x344 {
            self.msip = (value & 0x8) != 0;
            return;
        }
        self.csrs.insert(addr, value);
    }

    /// Load a word from memory (returns 0 for uninitialized).
    /// Byte-addressed: extracts the 32-bit word starting at the
    /// (4-byte aligned portion of the) given address.
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
    /// Load an unsigned byte at byte_addr.
    pub fn load_byte(&self, byte_addr: u32) -> u8 {
        let word = self.load_word(byte_addr & !3);
        let off = (byte_addr & 3) * 8;
        ((word >> off) & 0xFF) as u8
    }
    /// Load an unsigned halfword at byte_addr (which must be 2-aligned;
    /// the misaligned-load trap happens in `step` before this is called).
    pub fn load_halfword(&self, byte_addr: u32) -> u16 {
        let word = self.load_word(byte_addr & !3);
        let off = (byte_addr & 2) * 8;
        ((word >> off) & 0xFFFF) as u16
    }
    /// Store a byte at byte_addr — read-modify-write the containing word.
    pub fn store_byte(&mut self, byte_addr: u32, value: u8) {
        let word_addr = byte_addr / 4;
        let off = (byte_addr & 3) * 8;
        let mask = !(0xFFu32 << off);
        let old = *self.memory.get(&word_addr).unwrap_or(&0);
        let new_word = (old & mask) | ((value as u32) << off);
        self.memory.insert(word_addr, new_word);
        self.mem_writes.push((byte_addr, value as u32));
    }
    /// Store a halfword (16 bits) at byte_addr — read-modify-write.
    /// byte_addr must be 2-aligned.
    pub fn store_halfword(&mut self, byte_addr: u32, value: u16) {
        let word_addr = byte_addr / 4;
        let off = (byte_addr & 2) * 8;
        let mask = !(0xFFFFu32 << off);
        let old = *self.memory.get(&word_addr).unwrap_or(&0);
        let new_word = (old & mask) | ((value as u32) << off);
        self.memory.insert(word_addr, new_word);
        self.mem_writes.push((byte_addr, value as u32));
    }

    /// Vector to mtvec; save trapping PC to mepc; set mcause.
    /// `mtval` is left at its previous value (callers that care
    /// about mtval should use [`Cpu::take_trap_with_val`]).
    fn take_trap(&mut self, cause: u32) {
        self.take_trap_with_val(cause, 0);
    }

    /// Vector to mtvec; save trapping PC to mepc; set mcause and
    /// mtval; atomically save mstatus.MIE → MPIE and clear MIE.
    /// Used for sync exceptions (the simulator's `take_trap` wraps
    /// it with tval=0) and for interrupts.
    ///
    /// Vectored mtvec (mtvec[1:0] = 0b01): interrupts (cause bit 31
    /// set) vector to `(mtvec & ~0x3) + 4 * (cause & 0xF)`; sync
    /// exceptions go to the base regardless of mode.
    fn take_trap_with_val(&mut self, cause: u32, tval: u32) {
        self.write_csr(0x341, self.pc);     // mepc
        self.write_csr(0x342, cause);       // mcause
        self.write_csr(0x343, tval);        // mtval
        // mstatus.MIE → MPIE; mstatus.MIE ← 0.
        let mstatus = self.read_csr(0x300);
        let mie_bit = mstatus & 0x8;
        let cleared = mstatus & !0x88u32;
        let new_mstatus = cleared | (mie_bit << 4);
        self.write_csr(0x300, new_mstatus);
        // Compute vector target.
        let mtvec = self.read_csr(0x305);
        let base = mtvec & !0x3u32;
        let mode = mtvec & 0x3;
        let is_interrupt = (cause & 0x8000_0000) != 0;
        self.pc = if is_interrupt && mode == 1 {
            base + 4 * (cause & 0xF)
        } else {
            base
        };
    }

    /// MRET: restore mstatus.MIE from MPIE; set MPIE = 1; PC ← mepc.
    pub fn execute_mret(&mut self) {
        let mstatus = self.read_csr(0x300);
        let mpie_bit = mstatus & 0x80;                // bit 7
        let cleared = mstatus & !0x88u32;             // clear bits 3 and 7
        let new_mstatus = cleared | (mpie_bit >> 4) | 0x80;  // bit 7 → bit 3, MPIE = 1
        self.write_csr(0x300, new_mstatus);
        self.pc = self.read_csr(0x341);  // PC ← mepc
    }

    /// Pending+enabled M-mode interrupts: `mip & mie & MIE_M_MASK`.
    fn int_pending_enabled(&self) -> u32 {
        self.effective_mip() & self.read_csr(0x304) & 0x888
    }

    /// Should an interrupt fire this cycle?  True iff mstatus.MIE
    /// is set AND any M-mode interrupt is pending+enabled.
    fn interrupt_pending(&self) -> bool {
        let mstatus_mie = (self.read_csr(0x300) & 0x8) != 0;
        mstatus_mie && self.int_pending_enabled() != 0
    }

    /// Pick the highest-priority pending+enabled interrupt cause.
    /// Per spec: M-external > M-software > M-timer.
    fn interrupt_cause(&self) -> u32 {
        let pe = self.int_pending_enabled();
        if pe & 0x800 != 0 {
            0x8000_000B  // M-external
        } else if pe & 0x008 != 0 {
            0x8000_0003  // M-software
        } else {
            0x8000_0007  // M-timer
        }
    }

    /// Execute one instruction.  Fetches from `program[pc/4]`;
    /// if pc/4 is past the end of `program`, treats the fetched
    /// instruction as 0 (which decodes as illegal and triggers a
    /// trap).
    ///
    /// Interrupt-pending check happens FIRST — interrupts taken
    /// between instructions per the privileged-ISA spec, so a
    /// pending+enabled interrupt squashes the to-be-executed
    /// instruction and traps.  Same priority/cause as the hardware
    /// path.
    pub fn step(&mut self, program: &[u32]) {
        // Check for pending interrupts FIRST.  When an interrupt
        // fires, the in-flight instruction is squashed and mepc =
        // current PC (so MRET re-executes it).
        if self.interrupt_pending() {
            self.retired += 1;
            let cause = self.interrupt_cause();
            self.take_trap_with_val(cause, 0);
            return;
        }

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

        // SYSTEM ops (ECALL/EBREAK/MRET/WFI) — handled before the
        // generic ALU/branch dispatch.  WFI is a NOP without
        // external interrupts (per the privileged-ISA spec's
        // explicit allowance).
        match dec.system_op {
            SystemOp::Ecall  => { self.take_trap(11); return; }
            SystemOp::Ebreak => { self.take_trap(3); return; }
            SystemOp::Mret   => {
                self.execute_mret();              // restore MIE; PC ← mepc
                return;
            }
            SystemOp::Wfi    => {
                // NOP: advance PC and retire normally.
                self.pc = self.pc.wrapping_add(4);
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

        // Misaligned-load/store detection (mcause = 4 / 6).
        let h_mis = (alu_result & 0x1) != 0;
        let w_mis = (alu_result & 0x3) != 0;
        let mem_misaligned = match dec.mem_op {
            MemOp::Lh | MemOp::Lhu | MemOp::Sh => h_mis,
            MemOp::Lw | MemOp::Sw              => w_mis,
            _                                  => false,
        };
        if dec.mem_read && mem_misaligned {
            self.take_trap_with_val(4, alu_result);
            return;
        }
        if dec.mem_write && mem_misaligned {
            self.take_trap_with_val(6, alu_result);
            return;
        }

        // Memory access — proper byte-addressed sub-word semantics.
        if dec.mem_write {
            let addr = alu_result;
            let store_data = rs2_val;
            match dec.mem_op {
                MemOp::Sw => self.store_word(addr, store_data),
                MemOp::Sh => self.store_halfword(addr, store_data as u16),
                MemOp::Sb => self.store_byte(addr, store_data as u8),
                _         => {}
            }
        }
        let load_value: u32 = if dec.mem_read {
            match dec.mem_op {
                MemOp::Lb  => ((self.load_byte(alu_result) as i8) as i32) as u32,
                MemOp::Lh  => ((self.load_halfword(alu_result) as i16) as i32) as u32,
                MemOp::Lw  => self.load_word(alu_result),
                MemOp::Lbu => self.load_byte(alu_result) as u32,
                MemOp::Lhu => self.load_halfword(alu_result) as u32,
                _          => self.load_word(alu_result),
            }
        } else {
            0
        };

        // Compute next-PC (and detect misaligned-target trap)
        // before committing writeback, so the trap can suppress
        // both the writeback and the redirect — matching the
        // hardware (where `writeback_en` is gated by `!take_trap`).
        let take_branch = dec.opcode == Opcode::Branch && branch_taken;
        let take_jal    = dec.opcode == Opcode::Jal;
        let take_jalr   = dec.is_jalr;

        let prospective_target: u32 = if take_jalr {
            (rs1_val.wrapping_add(imm)) & !1u32
        } else if take_jal || take_branch {
            self.pc.wrapping_add(imm)
        } else {
            pc_plus_4
        };

        // Misaligned-target trap (mcause = 0): branch / JAL / JALR
        // whose target is not 4-byte aligned.  Suppresses writeback
        // and redirects to mtvec instead of the misaligned target.
        let attempts_redirect = take_branch || take_jal || take_jalr;
        let target_misaligned = (prospective_target & 0x3) != 0;
        if attempts_redirect && target_misaligned {
            self.take_trap_with_val(0, prospective_target);
            return;
        }

        // Writeback (only after we know we're not trapping).
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

        self.pc = prospective_target;
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
