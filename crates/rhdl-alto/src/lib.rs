//! `rhdl-alto` — Xerox Alto (1973) microengine implemented in RHDL.
//!
//! Tier C flagship core #2 per `tier-c-flagship-cores.md` §5.  The
//! Alto runs CPU instructions, display refresh, disk I/O, Ethernet
//! packets, and mouse input *all from a shared microengine* —
//! sixteen priority-ordered hardware tasks taking turns on one
//! horizontal-microcode pipeline.  Implementing the Alto in RHDL
//! demonstrates that the language can express the most aggressively
//! heterogeneous digital design ever shipped.
//!
//! ## What ships in Phases 1+2 (this crate's first PR)
//!
//! - [`isa`]         — 32-bit microinstruction format + ALU/F1/F2/BS
//!                      enums per the Alto Hardware Manual.
//! - [`alu`]         — pure-combinational kernel implementing the 16
//!                      Alto ALU functions.
//! - [`regfile`]     — R-register file (32 × 16 bits).  S-registers
//!                      (256 × 16 bits, 8 banks) are deferred to Phase 3.
//! - [`microcycle`]  — shared per-cycle execution kernel
//!                      (`compute_cycle`); used by both single-task
//!                      microengine and multi-task system.
//! - [`microengine`] — 2-stage MIF/MIE pipeline running a single
//!                      task from a 1024-microinstruction RAM.
//! - [`task_system`] — **the 16-task wakeup arbiter as an
//!                      [`rhdl_rule`] kernel** (Phase 2).  Each
//!                      Alto hardware task is one `#[rule]` method
//!                      with a `wakeup`-bit guard; the
//!                      `#[rule_kernel_attr]` macro generates a
//!                      priority-arbitrated scheduler from the
//!                      `#[rule(priority = N)]` annotations.
//!                      The most direct, BSV-natural expression
//!                      of the Alto's defining microarchitecture.
//!
//! Sufficient to execute hand-written Alto microcode that does ALU
//! ops on R-registers and stores results, AND to demonstrate the
//! full 16-task priority-arbitrated scheduling that defines the
//! Alto.  Disk task, Display task, etc. add per-task body
//! specialisation in later phases.
//!
//! ## What's deferred (per `tier-c-flagship-cores.md` §5.5)
//!
//! - **Phase 3**: Disk Sector + Disk Word tasks; boot the original
//!   Alto disk image far enough to reach the OS loader.  Also adds
//!   per-task body specialisation (Phase 2's task system has
//!   identical bodies modulo wakeup; Phase 3 starts diverging them).
//! - **Phase 4**: Display Word/Horizontal/Vertical tasks; render
//!   a 606×808 monochrome framebuffer.
//! - **Phase 5**: Mouse, Cursor, Keyboard tasks.
//! - **Phase 6**: Ethernet task (optional v1).
//! - **Phase 7**: Smalltalk-76 boot validation against ContrAlto.
//! - **Phase 8**: Book chapter + paper.
//!
//! Each phase ships in its own PR.  The phases are independent
//! enough that progress within a phase doesn't block the next.
//!
//! ## Crate layout decision
//!
//! `tier-c-flagship-cores.md` §5.7 specifies the deliverables go in
//! `crates/rhdl-fpga/src/alto/`.  This crate lives at
//! `crates/rhdl-alto/` instead, consistent with `rhdl-rv32i`'s
//! decision (PR #28): a CPU/microengine should not be bundled inside
//! the widget library — users who want widgets shouldn't transitively
//! pull in an Alto microengine.  The two locations would have produced
//! byte-identical hardware either way; this is a packaging decision.

pub mod alu;
pub mod diablo_disk;
pub mod disk_controller;
pub mod isa;
pub mod memory;
pub mod microcycle;
pub mod microengine;
pub mod regfile;
pub mod task_system;
