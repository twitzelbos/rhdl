//! Single-cycle RV32I core.
//!
//! Per `tier-c-flagship-cores.md` §3.5 Phase 1: a non-pipelined
//! single-cycle implementation that retires one instruction per
//! cycle.  Used as the executable specification against which the
//! Phase 2 pipelined version will be validated, and as the
//! lockstep reference for Spike cosimulation.
//!
//! ## Microarchitecture
//!
//! Per-cycle data flow:
//!
//! 1. Read `instr` from program memory at address `pc` (combinational).
//! 2. Decode `instr` via [`crate::decoder::decode`] → `DecodedInstruction`.
//! 3. Read source registers via the [`RegFile`] (combinational).
//! 4. Compute ALU result via [`crate::alu::alu`] (combinational).
//! 5. For loads/stores: drive the data-memory port with the ALU
//!    result as the address.
//! 6. Compute the writeback value (ALU / Mem / PC+4) per
//!    `decoded.writeback_src` and drive the register file's write
//!    port.
//! 7. Compute the next PC: PC+4, branch target, or jump target
//!    (JAL / JALR).
//! 8. At the next clock edge, PC and the register file commit.
//!
//! ## Memory interface
//!
//! v0.1 takes program memory as a const-generic `Bits<32>` array
//! input — i.e. the parent widget owns the memory and provides it
//! by value each cycle.  This is intentional for v0.1 to keep the
//! core's surface small; v0.2 will switch to a proper read-port
//! interface (per the plan's `RCStream` direction).
//!
//! Data memory is similarly handled via a typed input port.
//!
//! ## What's NOT in v0.1
//!
//! - No CSR file (deferred to Phase 3).
//! - No trap handling (`ECALL` / `EBREAK` / illegal-instruction
//!   traps are recognized by the decoder but the core sets a flag
//!   rather than vectoring).
//! - No 5-stage pipeline (Phase 2).
//! - No Spike lockstep harness (cross-cutting infrastructure).
//!
//! Today's `cpu` is a direct implementation of the per-cycle
//! datapath above — concise enough to read end-to-end.  It does
//! not yet drive an external memory; instead the unit-test wrapper
//! provides a fixed 32-instruction program memory and a small
//! data scratchpad.  See the `tests/` directory for runnable
//! examples.

use crate::alu::alu;
use crate::csr::{CsrFile, In as CsrIn};
use crate::decoder::decode;
use crate::isa::{AluSrc, BranchOp, CsrOp, MemOp, Opcode, SystemOp, WritebackSrc};
use crate::reg_file::{In as RegIn, RegFile};
use rhdl::prelude::*;
use rhdl_fpga::core::dff;

/// Inputs to the single-cycle core.
///
/// v0.1 treats program memory and data memory as combinational
/// inputs supplied by the parent widget — see the test fixtures
/// for how to drive these from a fixed program.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct In {
    /// Instruction word fetched at address `pc` (parent widget
    /// supplies this combinationally based on the PC output).
    pub instr: Bits<32>,
    /// Data-memory read response — the 32-bit word at the data
    /// address driven by the previous cycle's load (or this
    /// cycle's address if the parent's memory is combinational).
    pub mem_rdata: Bits<32>,
}

/// Outputs from the single-cycle core.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct Out {
    /// Current program counter — the parent uses this to drive
    /// the next-cycle `instr` input.
    pub pc: Bits<32>,
    /// Data-memory write address (only meaningful when `mem_write`).
    pub mem_addr: Bits<32>,
    /// Data-memory write data (only meaningful when `mem_write`).
    pub mem_wdata: Bits<32>,
    /// Data-memory write enable.
    pub mem_write: bool,
    /// Data-memory read enable.
    pub mem_read: bool,
    /// Memory-access width (B/H/W; signed/unsigned for loads).
    pub mem_op: MemOp,
    /// True iff the decoder rejected the current instruction.
    /// The CPU continues with the PC advancing as if it were a NOP;
    /// the parent is responsible for trapping on this if desired.
    pub illegal: bool,
}

/// Single-cycle RV32I CPU widget.  Composes the [`RegFile`] sub-
/// circuit with a PC register; the decoder, ALU, and memory-control
/// signals are all combinational kernels.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct Cpu {
    /// Program counter — bumped by 4 each cycle unless the
    /// instruction is a taken branch / jump / trap.
    pc: dff::DFF<Bits<32>>,
    /// 32-entry register file (x0 hardwired to zero).
    rf: RegFile,
    /// M-mode CSR file (Phase 3).
    csrs: CsrFile,
}

impl Default for Cpu {
    fn default() -> Self {
        Self {
            pc: dff::DFF::new(bits::<32>(0)),
            rf: RegFile::default(),
            csrs: CsrFile::default(),
        }
    }
}

impl SynchronousIO for Cpu {
    type I = In;
    type O = Out;
    type Kernel = cpu_kernel;
}

/// Decide whether a branch is taken given the comparator op and
/// the two register values.
#[kernel]
pub fn branch_taken(op: BranchOp, a: Bits<32>, b: Bits<32>) -> bool {
    let a_signed: SignedBits<32> = a.as_signed();
    let b_signed: SignedBits<32> = b.as_signed();
    match op {
        BranchOp::Eq  => a == b,
        BranchOp::Ne  => a != b,
        BranchOp::Lt  => a_signed < b_signed,
        BranchOp::Ge  => a_signed >= b_signed,
        BranchOp::Ltu => a < b,
        BranchOp::Geu => a >= b,
    }
}

/// Format a load result from the raw memory word, applying the
/// width and sign-extension implied by `op`.  v0.1 assumes the
/// memory returns the full 32-bit word and we extract the relevant
/// byte/half from it; misaligned loads are not yet handled.
#[kernel]
pub fn load_format(op: MemOp, raw: Bits<32>) -> Bits<32> {
    let raw_signed: SignedBits<32> = raw.as_signed();
    match op {
        MemOp::Lb  => ((raw_signed << 24) >> 24).as_unsigned(),
        MemOp::Lh  => ((raw_signed << 16) >> 16).as_unsigned(),
        MemOp::Lw  => raw,
        MemOp::Lbu => raw & bits::<32>(0xFF),
        MemOp::Lhu => raw & bits::<32>(0xFFFF),
        // Stores don't load; but we have to return something
        // since the field always has a value.  Pass through.
        MemOp::Sb  => raw,
        MemOp::Sh  => raw,
        MemOp::Sw  => raw,
    }
}

#[kernel]
/// Single-cycle CPU kernel.  Does the entire fetch-decode-execute-
/// memory-writeback datapath in one combinational sweep, then
/// commits the new PC and register-file state at the clock edge.
pub fn cpu_kernel(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    let mut o = Out::dont_care();

    // Decode the current instruction.
    let dec = decode(i.instr);

    // Read source registers via the RegFile sub-widget — the
    // framework drives its inputs via `d.rf` and presents its
    // outputs via `q.rf` combinationally.
    let rs1_val: Bits<32> = q.rf.rdata1;
    let rs2_val: Bits<32> = q.rf.rdata2;

    // Pick ALU operand A: PC for AUIPC, otherwise rs1.
    let alu_a: Bits<32> = if dec.opcode == Opcode::Auipc {
        q.pc
    } else {
        rs1_val
    };
    // Pick ALU operand B: immediate or rs2.
    let alu_b: Bits<32> = if dec.alu_src == AluSrc::Imm {
        dec.imm
    } else {
        rs2_val
    };

    let alu_result: Bits<32> = alu(dec.alu_op, alu_a, alu_b);

    // Memory address is the ALU result for loads/stores.
    o.mem_addr = alu_result;
    o.mem_wdata = rs2_val; // store data comes from rs2
    o.mem_write = dec.mem_write;
    o.mem_read = dec.mem_read;
    o.mem_op = dec.mem_op;

    // Branch decision and next PC computation.
    let pc_plus_4: Bits<32> = q.pc + bits::<32>(4);
    let branch_t: bool = branch_taken(dec.branch_op, rs1_val, rs2_val);
    let take_branch: bool = (dec.opcode == Opcode::Branch) && branch_t;

    let branch_target: Bits<32> = q.pc + dec.imm;
    let jal_target: Bits<32>    = q.pc + dec.imm;
    let jalr_target: Bits<32>   = (rs1_val + dec.imm) & bits::<32>(0xFFFF_FFFE);

    let next_pc: Bits<32> = if dec.is_jalr {
        jalr_target
    } else if dec.opcode == Opcode::Jal {
        jal_target
    } else if take_branch {
        branch_target
    } else {
        pc_plus_4
    };

    // CSR access — combinational read of the addressed CSR, plus
    // computation of the new CSR value based on the CSR-op.  The
    // CSR-instruction source is rs1 (for register variants) or
    // the zero-extended rs1 field (for immediate variants).
    let csr_rdata: Bits<32> = q.csrs.rdata;
    let csr_uimm: Bits<32> = dec.rs1.resize();
    let csr_src: Bits<32> = match dec.csr_op {
        CsrOp::ReadWriteImm => csr_uimm,
        CsrOp::ReadSetImm   => csr_uimm,
        CsrOp::ReadClearImm => csr_uimm,
        _                    => rs1_val,
    };
    let csr_new_value: Bits<32> = match dec.csr_op {
        CsrOp::ReadWrite     => csr_src,
        CsrOp::ReadWriteImm  => csr_src,
        CsrOp::ReadSet       => csr_rdata | csr_src,
        CsrOp::ReadSetImm    => csr_rdata | csr_src,
        CsrOp::ReadClear     => csr_rdata & !csr_src,
        CsrOp::ReadClearImm  => csr_rdata & !csr_src,
        CsrOp::None          => csr_rdata,
    };
    // CSRRS / CSRRC with rs1 = x0 is a pure read (no write).
    // CSRRSI / CSRRCI with uimm = 0 is also a pure read.
    let csr_writes: bool = match dec.csr_op {
        CsrOp::None         => false,
        CsrOp::ReadWrite    => true,
        CsrOp::ReadWriteImm => true,
        CsrOp::ReadSet      => dec.rs1 != bits::<5>(0),
        CsrOp::ReadSetImm   => dec.rs1 != bits::<5>(0),
        CsrOp::ReadClear    => dec.rs1 != bits::<5>(0),
        CsrOp::ReadClearImm => dec.rs1 != bits::<5>(0),
    };

    // Misaligned-target trap detection.  RV32I requires:
    //   - branch / JAL: target must be 4-byte aligned (bits[1:0] = 00)
    //   - JALR:         after masking bit 0, target must be 4-byte
    //                   aligned (bit 1 must also be 0)
    // When the trap fires, mcause = 0 (Instruction address misaligned)
    // and mtval = the misaligned target.  The trapping instruction
    // does not commit (writeback suppressed; PC vectors to mtvec).
    let prospective_target: Bits<32> = if dec.is_jalr {
        jalr_target
    } else if dec.opcode == Opcode::Jal {
        jal_target
    } else {
        branch_target
    };
    let attempts_redirect: bool = dec.is_jalr
        || (dec.opcode == Opcode::Jal)
        || take_branch;
    let target_misaligned: bool = (prospective_target & bits::<32>(0x3)) != bits::<32>(0);
    let take_misaligned: bool = attempts_redirect && target_misaligned;

    // Trap handling.  ECALL/EBREAK/illegal-instruction/misaligned
    // all vector to mtvec; mcause distinguishes:
    //   0 — Instruction address misaligned
    //   2 — Illegal instruction
    //   3 — Breakpoint (EBREAK)
    //  11 — Environment call from M-mode (ECALL)
    // MRET is the inverse: PC ← mepc.  WFI is a NOP.
    let take_ecall:      bool = dec.system_op == SystemOp::Ecall;
    let take_ebreak:     bool = dec.system_op == SystemOp::Ebreak;
    let take_illegal:    bool = dec.illegal;
    let take_trap:       bool = take_ecall || take_ebreak || take_illegal || take_misaligned;
    let take_mret:       bool = dec.system_op == SystemOp::Mret;
    let trap_cause: Bits<32> = if take_misaligned {
        bits::<32>(0)
    } else if take_illegal {
        bits::<32>(2)
    } else if take_ebreak {
        bits::<32>(3)
    } else {
        bits::<32>(11)
    };
    let trap_val: Bits<32> = if take_misaligned {
        prospective_target
    } else {
        bits::<32>(0)
    };

    // Pick writeback value.
    let mem_value: Bits<32> = load_format(dec.mem_op, i.mem_rdata);
    let writeback_value: Bits<32> = match dec.writeback_src {
        WritebackSrc::None    => bits::<32>(0),
        WritebackSrc::Alu     => alu_result,
        WritebackSrc::Mem     => mem_value,
        WritebackSrc::PcPlus4 => pc_plus_4,
        WritebackSrc::Csr     => csr_rdata,
    };
    // Suppress writeback on a trap (the in-flight instruction is
    // squashed in favour of the trap entry).
    let writeback_en: bool = !take_trap
        && (dec.writeback_src != WritebackSrc::None)
        && (dec.rd != bits::<5>(0));

    // Drive the register file's input port.
    d.rf = RegIn {
        raddr1: dec.rs1,
        raddr2: dec.rs2,
        waddr: dec.rd,
        wdata: writeback_value,
        wen: writeback_en,
    };

    // Drive the CSR file.  CSR-instruction port is suppressed on
    // a trap (same as register writeback).  The trap port carries
    // the saved PC and cause when a trap fires.
    d.csrs = CsrIn {
        raddr: dec.csr_addr,
        waddr: dec.csr_addr,
        wdata: csr_new_value,
        wen: !take_trap && csr_writes,
        trap_en: take_trap,
        trap_pc: q.pc,
        trap_cause,
        trap_val,
    };

    // Commit next PC — trap entry overrides everything else;
    // MRET overrides next_pc to redirect to mepc.
    let trap_target: Bits<32> = q.csrs.mtvec;
    let mret_target: Bits<32> = q.csrs.mepc;
    let next_pc_with_trap: Bits<32> = if take_trap {
        trap_target
    } else if take_mret {
        mret_target
    } else {
        next_pc
    };
    d.pc = next_pc_with_trap;

    o.pc = q.pc;
    o.illegal = dec.illegal;

    if cr.reset.any() {
        d.pc = bits::<32>(0);
        o.pc = bits::<32>(0);
        o.mem_write = false;
        o.mem_read = false;
        o.illegal = false;
    }
    (o, d)
}
