//! Inter-stage pipeline register bundles for the 5-stage RV32I core.
//!
//! Each bundle carries the data and control signals one stage
//! produces and the next stage consumes.  A `valid` flag rides
//! along so stalls and squashes can mark a slot as a bubble (the
//! stage downstream sees a NOP-equivalent and produces no
//! observable side effect).
//!
//! The pipeline is:
//!
//! ```text
//!  Fetch  →  IfId  →  Decode  →  IdEx  →  Execute  →  ExMem  →  Memory  →  MemWb  →  Writeback
//!                                                       │                       │
//!                                                       └──forward path─────────┘
//!                                                       (forwarding into Execute)
//! ```
//!
//! Per `tier-c-flagship-cores.md` §3.4, the long-term direction is
//! `RCStream`-typed pipeline registers; v0.2 uses plain `Digital`
//! bundles (no ready/valid handshaking — the pipeline runs at full
//! throughput, hazards are managed by stall/squash signals from
//! the [`HazardUnit`]).

use crate::isa::{AluOp, AluSrc, BranchOp, CsrOp, MemOp, Opcode, SystemOp, WritebackSrc};
use rhdl::prelude::*;

/// IF/ID — the slot between Fetch and Decode.
///
/// Carries the raw instruction word the Fetch stage just read from
/// program memory, plus the PC that addressed it (needed downstream
/// for branch-relative computations and for the JAL/JALR return
/// address).
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub struct IfId {
    /// The PC that selected this instruction.
    pub pc: Bits<32>,
    /// The 32-bit instruction word.
    pub instr: Bits<32>,
    /// `true` if this slot holds a real instruction; `false` if
    /// it's a bubble (stall or squash).
    pub valid: bool,
}

/// ID/EX — the slot between Decode and Execute.
///
/// Carries the decoder's full control word plus the two source-
/// register values the Decode stage read out of the register file.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub struct IdEx {
    /// The PC that selected this instruction.
    pub pc: Bits<32>,
    /// The major opcode class.
    pub opcode: Opcode,
    /// Destination register (5 bits).
    pub rd: Bits<5>,
    /// Source-register-1 number (used by the forwarding mux).
    pub rs1: Bits<5>,
    /// Source-register-2 number.
    pub rs2: Bits<5>,
    /// Sign-extended immediate.
    pub imm: Bits<32>,
    /// ALU operation.
    pub alu_op: AluOp,
    /// Where ALU operand B comes from (register or immediate).
    pub alu_src: AluSrc,
    /// Branch condition (only meaningful when opcode == Branch).
    pub branch_op: BranchOp,
    /// Memory access width (only meaningful for Load / Store).
    pub mem_op: MemOp,
    /// Writeback selector.
    pub writeback_src: WritebackSrc,
    /// True if this is a memory write.
    pub mem_write: bool,
    /// True if this is a memory read.
    pub mem_read: bool,
    /// True if this is JAL or JALR.
    pub jump: bool,
    /// True if specifically JALR (target = rs1+imm, not pc+imm).
    pub is_jalr: bool,
    /// rs1's value as read from the register file (pre-forwarding).
    pub rs1_val: Bits<32>,
    /// rs2's value as read from the register file (pre-forwarding).
    pub rs2_val: Bits<32>,
    /// CSR-access operation (Phase 3 pipelined CSR support).
    pub csr_op: CsrOp,
    /// CSR address (12-bit funct12 for CSR instructions).
    pub csr_addr: Bits<12>,
    /// SYSTEM-opcode subtype (ECALL / EBREAK).
    pub system_op: SystemOp,
    /// True for a real instruction; false for a bubble.
    pub valid: bool,
}

/// EX/MEM — the slot between Execute and Memory.
///
/// Carries the ALU result (which is also the load/store address
/// for memory ops), the rs2 value (for store data), the writeback
/// destination, and the writeback control signals.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub struct ExMem {
    /// Destination register.
    pub rd: Bits<5>,
    /// ALU result — also the load/store address for memory ops.
    pub alu_result: Bits<32>,
    /// rs2 value — store data for stores.
    pub rs2_val: Bits<32>,
    /// Memory access width.
    pub mem_op: MemOp,
    /// Writeback selector.
    pub writeback_src: WritebackSrc,
    /// True iff memory write.
    pub mem_write: bool,
    /// True iff memory read.
    pub mem_read: bool,
    /// PC + 4 for JAL/JALR writeback.
    pub pc_plus_4: Bits<32>,
    /// CSR address (12-bit) — used by Writeback to drive the CSR
    /// file's write port.
    pub csr_addr: Bits<12>,
    /// New value to write to the CSR (computed in Execute from
    /// the CSR-op semantics).
    pub csr_new_value: Bits<32>,
    /// True iff this instruction needs to commit a CSR write at
    /// the Writeback stage.  False for non-CSR instructions and
    /// for CSRRS/CSRRC with rs1 = x0 (which are pure reads).
    pub csr_writes: bool,
    /// True for a real instruction; false for a bubble.
    pub valid: bool,
}

/// MEM/WB — the slot between Memory and Writeback.
///
/// Carries the writeback value (already selected from ALU/Mem/PC+4)
/// and the destination register.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub struct MemWb {
    /// Destination register.
    pub rd: Bits<5>,
    /// Final writeback value (already mux-selected by the Memory
    /// stage from `alu_result` / `mem_value` / `pc_plus_4` per
    /// the `writeback_src`).
    pub writeback_value: Bits<32>,
    /// True iff a writeback should occur.
    pub writeback_en: bool,
    /// CSR address (12-bit) — driven onto the CSR file's write
    /// port by Writeback.
    pub csr_addr: Bits<12>,
    /// CSR new value carried forward from Execute.
    pub csr_new_value: Bits<32>,
    /// True iff this instruction commits a CSR write.
    pub csr_writes: bool,
    /// True for a real instruction; false for a bubble.
    pub valid: bool,
}

/// Forwarding selector for one of the two ALU operand inputs.
///
/// Computed by [`HazardUnit`] each cycle and consumed by the
/// Execute stage.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub enum ForwardSrc {
    /// No forwarding — use the value from ID/EX.
    #[default]
    None,
    /// Forward from the EX/MEM register's `alu_result`.
    ExMem,
    /// Forward from the MEM/WB register's `writeback_value`.
    MemWb,
}
