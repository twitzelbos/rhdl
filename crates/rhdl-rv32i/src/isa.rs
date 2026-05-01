//! RV32I instruction set architecture types.
//!
//! Per `tier-c-flagship-cores.md` §3.2, this crate implements the
//! base RV32I integer ISA — 47 instructions — plus the M-mode
//! privileged subset.  This module defines the encoding-format
//! types and the per-instruction enums that the decoder produces
//! and the executor consumes.
//!
//! Spec reference: *The RISC-V Instruction Set Manual, Volume I:
//! Unprivileged ISA*, version 20240411 (Chapter 2 — RV32I Base).

use rhdl::prelude::*;

/// Major opcode classes (bits 6:0 of an instruction word).  These
/// are the seven distinct opcodes plus three system opcodes that
/// RV32I uses.  Names follow the spec.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub enum Opcode {
    /// Catch-all for any opcode the decoder doesn't recognize.
    #[default]
    Illegal,
    /// `0110011` — R-type ALU (`ADD`, `SUB`, `AND`, `OR`, `XOR`,
    /// `SLL`, `SRL`, `SRA`, `SLT`, `SLTU`).
    OpReg,
    /// `0010011` — I-type ALU (`ADDI`, `ANDI`, `ORI`, `XORI`,
    /// `SLLI`, `SRLI`, `SRAI`, `SLTI`, `SLTIU`).
    OpImm,
    /// `0000011` — Loads (`LB`, `LH`, `LW`, `LBU`, `LHU`).
    Load,
    /// `0100011` — Stores (`SB`, `SH`, `SW`).
    Store,
    /// `1100011` — Branches (`BEQ`, `BNE`, `BLT`, `BGE`, `BLTU`,
    /// `BGEU`).
    Branch,
    /// `1100111` — I-type `JALR`.
    Jalr,
    /// `1101111` — J-type `JAL`.
    Jal,
    /// `0110111` — U-type `LUI`.
    Lui,
    /// `0010111` — U-type `AUIPC`.
    Auipc,
    /// `1110011` — System (`ECALL`, `EBREAK`, CSR-access in v0.2).
    System,
    /// `0001111` — `FENCE` (treated as NOP in this in-order
    /// single-hart core).
    MiscMem,
}

/// ALU operation selected by the decoder.  Drives the [`Alu`]
/// kernel.  Encodes the cross-product of `funct3` × the relevant
/// `funct7` bit (which distinguishes ADD/SUB and SRL/SRA).
///
/// `Add` / `Sub` are also used for branch-condition computation
/// and for load/store address calculation — the executor selects
/// the ALU op to use, not the decoder.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub enum AluOp {
    #[default]
    Add,
    Sub,
    Sll,
    Slt,
    Sltu,
    Xor,
    Srl,
    Sra,
    Or,
    And,
    /// Pseudo-op used by LUI: pass through the second operand.
    /// Implemented as `0 + imm`; named separately so the executor
    /// can document intent.
    Pass,
}

/// Branch condition encoded by the `funct3` field of a B-type
/// instruction.  Drives the branch-comparator kernel.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub enum BranchOp {
    #[default]
    Eq,
    Ne,
    Lt,
    Ge,
    Ltu,
    Geu,
}

/// Memory-access width and signedness encoded by the `funct3`
/// field of a load/store.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub enum MemOp {
    /// `LB` — load 8-bit signed.
    #[default]
    Lb,
    /// `LH` — load 16-bit signed.
    Lh,
    /// `LW` — load 32-bit.
    Lw,
    /// `LBU` — load 8-bit unsigned.
    Lbu,
    /// `LHU` — load 16-bit unsigned.
    Lhu,
    /// `SB` — store low 8 bits.
    Sb,
    /// `SH` — store low 16 bits.
    Sh,
    /// `SW` — store 32 bits.
    Sw,
}

/// Source for the second ALU operand: a register or an immediate.
/// Selected per opcode class.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub enum AluSrc {
    #[default]
    Reg,
    Imm,
}

/// What writes back to the register file at the end of the cycle.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub enum WritebackSrc {
    /// No writeback (stores, branches, FENCE).
    #[default]
    None,
    /// ALU result.
    Alu,
    /// Loaded value from memory.
    Mem,
    /// PC + 4 (return address for JAL/JALR).
    PcPlus4,
    /// Pre-modify value of the CSR (for `CSRR{W,S,C}{I,}`).
    Csr,
}

/// CSR-access operation encoded by the `funct3` field of a SYSTEM
/// instruction (when the instruction is *not* `ECALL`/`EBREAK`).
///
/// All forms read the CSR into rd, then optionally write back to
/// the CSR using the operand (rs1 value or zero-extended 5-bit
/// immediate).  The "Imm" variants take the source from the rs1
/// register field (zero-extended) instead of the actual rs1
/// register's value.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub enum CsrOp {
    /// Not a CSR instruction.
    #[default]
    None,
    /// `CSRRW` — `rd ← csr; csr ← rs1`.
    ReadWrite,
    /// `CSRRS` — `rd ← csr; csr ← csr | rs1` (write only if rs1 != x0).
    ReadSet,
    /// `CSRRC` — `rd ← csr; csr ← csr & ~rs1` (write only if rs1 != x0).
    ReadClear,
    /// `CSRRWI` — `rd ← csr; csr ← zext(uimm)`.
    ReadWriteImm,
    /// `CSRRSI` — `rd ← csr; csr ← csr | zext(uimm)` (write only if uimm != 0).
    ReadSetImm,
    /// `CSRRCI` — `rd ← csr; csr ← csr & ~zext(uimm)` (write only if uimm != 0).
    ReadClearImm,
}

/// SYSTEM-opcode variants other than CSR access.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub enum SystemOp {
    /// Not an ECALL or EBREAK (this includes CSR instructions —
    /// see [`CsrOp`]).
    #[default]
    None,
    /// `ECALL` — environment call (M-mode here).
    Ecall,
    /// `EBREAK` — debugger breakpoint.
    Ebreak,
}

/// Decoder output — every signal the executor and writeback stages
/// need.  This is the canonical "control word" for the single-cycle
/// (and later 5-stage pipelined) implementation.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub struct DecodedInstruction {
    /// The major opcode class.
    pub opcode: Opcode,
    /// Destination register (5 bits — register x0..x31).  Always
    /// well-defined; for opcodes without a writeback the executor
    /// suppresses the write via [`WritebackSrc::None`].
    pub rd: Bits<5>,
    /// First source register.
    pub rs1: Bits<5>,
    /// Second source register.
    pub rs2: Bits<5>,
    /// Sign-extended immediate value (32 bits).  The decoder
    /// produces the correct value for the opcode class (I-type,
    /// S-type, B-type, U-type, or J-type encoding).
    pub imm: Bits<32>,
    /// Selected ALU operation.
    pub alu_op: AluOp,
    /// What feeds the second ALU operand.
    pub alu_src: AluSrc,
    /// Branch condition (only meaningful when `opcode == Branch`).
    pub branch_op: BranchOp,
    /// Memory access width (only meaningful for `Load` / `Store`).
    pub mem_op: MemOp,
    /// What writes back to the register file.
    pub writeback_src: WritebackSrc,
    /// True if the instruction is a memory write (store).
    pub mem_write: bool,
    /// True if the instruction is a memory read (load).
    pub mem_read: bool,
    /// True if this instruction triggers a control-flow change
    /// (JAL, JALR, or a taken Branch).  For Branch, the executor
    /// computes the actual taken/not-taken decision; the decoder
    /// only flags that this is a control-flow opcode.
    pub jump: bool,
    /// True if JALR specifically (which uses rs1+imm rather than
    /// PC+imm for the target).
    pub is_jalr: bool,
    /// True if this instruction is illegal (decoder couldn't match
    /// any RV32I encoding).
    pub illegal: bool,
    /// CSR-access operation (only meaningful when `opcode == System`
    /// and the instruction is one of `CSRR{W,S,C}{I,}`).
    pub csr_op: CsrOp,
    /// CSR address (12-bit `funct12` field for CSR instructions).
    pub csr_addr: Bits<12>,
    /// SYSTEM opcode sub-type (`ECALL` / `EBREAK` / none).
    pub system_op: SystemOp,
}
