//! `rhdl-rv32i` — RISC-V 32-bit base integer ISA, in RHDL.
//!
//! Tier C flagship core per `tier-c-flagship-cores.md` §3.  This is
//! the v0.1 starter: foundational types, decoder, ALU, register file.
//! The five-stage pipeline, CSRs, and riscv-tests harness are
//! deferred to follow-on PRs.
//!
//! ## What ships in v0.1
//!
//! - [`isa`] — instruction-format types (`Opcode`, `AluOp`,
//!   `BranchOp`, `MemOp`, `AluSrc`, `WritebackSrc`, the
//!   `DecodedInstruction` control word).
//! - [`decoder::decode`] — pure combinational kernel that decodes
//!   any 32-bit instruction word into a `DecodedInstruction`.
//!   Covers all 47 RV32I instructions plus the FENCE / ECALL /
//!   EBREAK system instructions.
//! - [`alu::alu`] — pure combinational kernel implementing every
//!   ALU operation needed for R-type, I-type, branch comparators,
//!   and address calculation.
//! - [`reg_file`] — synchronous 32×32-bit register file with x0
//!   hardwired to zero.
//! - [`cpu`] — single-cycle CPU widget composing the above with
//!   a simple program memory and data memory.
//!
//! ## What's deferred (per tier-c-flagship-cores.md §3.5)
//!
//! - **Phase 2**: 5-stage pipeline (Fetch / Decode / Execute /
//!   Memory / Writeback) with hazard detection and forwarding.
//!   ~6-8 weeks of focused work.
//! - **Phase 3**: M-mode privileged extensions and CSRs.
//!   ~2-3 weeks.
//! - **Phase 4**: Documentation chapter and conference paper.
//!   ~3-4 weeks.
//!
//! Plus the validation infrastructure (riscv-tests harness,
//! Spike lockstep cosimulation) which is a cross-cutting effort
//! shared across all three Tier C cores.
//!
//! ## Crate layout decision
//!
//! `tier-c-flagship-cores.md` §3.7 specifies the deliverables go
//! in `crates/rhdl-fpga/src/rv32i/`.  This crate lives at
//! `crates/rhdl-rv32i/` instead, on the user's instinct that a
//! CPU core should not be bundled inside the widget library —
//! users who want widgets shouldn't transitively pull in a CPU.
//! The two locations would have produced byte-identical hardware
//! either way; this is a packaging decision.

pub mod alu;
pub mod compliance;
pub mod cpu;
pub mod csr;
pub mod decoder;
pub mod hazard;
pub mod isa;
pub mod pipeline;
pub mod pipelined;
pub mod reg_file;
pub mod sim;
