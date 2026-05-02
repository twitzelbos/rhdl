//! Alto microengine — the 2-stage horizontal-microcode pipeline.
//!
//! Per the *Alto Hardware Manual* §2 and §3, the Alto microengine
//! is a 2-stage pipeline:
//!
//! - **MIF** — Microinstruction Fetch: reads the next-cycle MPC
//!   from the running task, indexes the microcode RAM, and
//!   delivers the 32-bit microinstruction to MIE.
//! - **MIE** — Microinstruction Execute: drives BUS from the BS
//!   field, runs the ALU (selected by ALUF), latches L (if L_LOAD)
//!   and T (if T_LOAD), updates R (when an F1/F2 commands an R-load),
//!   computes the next MPC (combining the NEXT field with any F2
//!   modifiers), and yields if F1 = TASK / BLOCK.
//!
//! Phase 1 ships the universal microengine running a single task
//! (Task 0) — there's no arbiter yet, so MPC is just a 10-bit DFF
//! that increments by `next` each cycle.  Tasks come in Phase 2.
//!
//! ## State (DFFs in this widget)
//!
//! - `mpc`     — Microprogram Counter (10 bits → 1024-microinstruction RAM).
//! - `t`       — T register (16 bits).
//! - `l`       — L register (16 bits).
//! - `regs`    — R-register file (sub-widget).
//! - `urom`    — Microcode RAM/ROM (sub-widget; for Phase 1 we use a
//!   1024-entry array of packed 32-bit microinstructions, indexed
//!   by MPC, exposed as a combinational read port).
//!
//! The microcode is loaded by the parent at construction time
//! (passed as a `[u32; 1024]` array).

use crate::alu::{alu, AluOut};
use crate::isa::{
    AluFunction, BusSource, F1Function, F2Function, Microinstruction,
};
use crate::regfile::{In as RegIn, Out as RegOut, RegFile};
use rhdl::prelude::*;
use rhdl_fpga::core::dff;

/// Inputs to the microengine.
///
/// Phase 3.5 ownership change: MPC is now owned **externally**
/// (by task_system in AltoChip composition, or by the test harness).
/// The microengine receives the current MPC and the corresponding
/// microinstruction each cycle, and returns the next MPC for the
/// owner to commit.  This avoids the 1-cycle alignment issue when
/// composing with a BRAM-backed microcode RAM, and lets the
/// task arbiter drive different tasks' MPCs into the same engine.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct In {
    /// Current cycle's microprogram counter — fetched from the owner
    /// (task_system or test harness).  10 bits in Phase 3.5 (bank 0
    /// only); will widen to 11 bits when bank switching ships.
    pub mpc: Bits<10>,
    /// 32-bit microinstruction at `mpc`.  Owner indexes microcode RAM
    /// with `mpc` (with appropriate latency handling) and feeds the
    /// result here.
    pub instr: Bits<32>,
    /// Constant-ROM value at index `(RSEL[4:0] << 3) | BS[2:0]`.
    /// Owner (AltoChip) is responsible for decoding the index from
    /// `instr`, indexing the constant ROM, and feeding the value
    /// here combinationally.  When `F1 = Constant`, the engine drives
    /// BUS from this value instead of the BS-selected source.
    pub constant_value: Bits<16>,
    /// Memory data read from `memory[MAR]` (1-cycle BRAM latency
    /// from the address presented last cycle).  Drives BUS when
    /// `BS = MemoryData`.
    pub mem_read_data: Bits<16>,
    /// Which task is running this cycle (0..15).  Reserved for
    /// per-task F1/F2 dispatch — currently unused by the kernel
    /// (universal codes only) but plumbed for the next phase.
    pub current_task: Bits<4>,
    /// Disk's current rotational word data (combinational from
    /// disk's `current_word_data` output).  Used by F2=DiskWordTransfer
    /// in Disk Word task to write the word to memory[KCWA].
    pub disk_word_data: Bits<16>,
    /// KCWA register value (combinational from controller's q.kcwa).
    /// The address memory[KCWA] is the destination for per-word DMA.
    pub kcwa: Bits<16>,
}

/// Outputs from the microengine.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct Out {
    /// Echo of the input MPC for trace.  This is the address whose
    /// microinstruction was processed this cycle.
    pub mpc: Bits<10>,
    /// Echo of the input current_task for trace + lockstep.
    pub current_task: Bits<4>,
    /// Next MPC the owner should commit (or pass to the next task).
    /// Computed from the NEXT field plus any F2-driven modifications.
    pub next_mpc: Bits<10>,
    /// Current T register value (visible for tests / lockstep).
    pub t: Bits<16>,
    /// Current L register value.
    pub l: Bits<16>,
    /// Current 16-bit BUS this cycle (for trace / lockstep).
    pub bus: Bits<16>,
    /// Current ALU result (for trace / lockstep).
    pub alu_result: Bits<16>,
    /// Memory address to drive into the memory subsystem this cycle.
    /// Equals the engine's MAR (which gets updated next cycle if
    /// F2 = LoadMar is asserted).
    pub mem_address: Bits<16>,
    /// Memory write enable — asserted when F2 = WriteMd.
    pub mem_write_en: bool,
    /// Memory write data — equals BUS when `mem_write_en`.
    pub mem_write_data: Bits<16>,
    /// Disk-controller register address (3 bits → registers 0-5).
    /// Sourced from RSEL[2:0] of the current instruction.
    pub disk_ctrl_addr: Bits<3>,
    /// Disk-controller write enable — true when F1 = DiskCtrlWrite
    /// AND current_task == 4 (Disk Sector).
    pub disk_ctrl_write_en: bool,
    /// Disk-controller write data — equals BUS when write_en.
    pub disk_ctrl_write_data: Bits<16>,
    /// True when the engine just consumed the disk's current word
    /// (F2=DiskWordTransfer in Disk Word task).  Routed to the disk's
    /// `word_consumed` input so the disk advances its position +
    /// decrements transfer_remaining only when DMA actually happens.
    pub disk_word_consumed: bool,
    /// True when this cycle's microinstruction has F1=TaskYield (TASK).
    /// Per the *Alto Hardware Manual* §2.4: this is the only signal
    /// that triggers task arbitration — task switches happen ONLY on
    /// F1=TASK, not per-cycle.  AltoChip uses this to gate its
    /// `current_task` latch.
    pub task_yield: bool,
    /// Instruction Register — current Nova instruction (Emulator task).
    pub ir: Bits<16>,
}

/// The Alto microengine widget.
///
/// **Phase 3.5**: the MPC is no longer owned internally — it's an
/// input.  Internal state remaining: T, L, R-register file.
/// AltoChip and the standalone microengine tests both supply MPC
/// externally (task_system's per-task MPCs in AltoChip;
/// a test-harness u16 in standalone tests).
#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct Microengine {
    /// T register (auxiliary ALU operand).
    t: dff::DFF<Bits<16>>,
    /// L register (latched ALU result).
    l: dff::DFF<Bits<16>>,
    /// R-register file.
    regs: RegFile,
    /// Memory Address Register — holds the current memory address
    /// for read/write ops.  Updated by F2 = LoadMar.
    mar: dff::DFF<Bits<16>>,
    /// Instruction Register — holds the current Nova instruction
    /// being executed by the Emulator task.  Updated by F2 = LoadIr
    /// in Emulator (task 0); other tasks don't touch it.
    ir: dff::DFF<Bits<16>>,
}

impl SynchronousIO for Microengine {
    type I = In;
    type O = Out;
    type Kernel = microengine_kernel;
}

#[kernel]
/// One microinstruction-execution cycle.
///
/// Phase 1 implements the universal-task microinstruction semantics:
/// drive BUS from BS, run the ALU, optionally latch T and L,
/// compute the next MPC.  Task-specific F1/F2 codes are no-ops
/// in Phase 1 (we model only the universal codes: NOP, shifts on L,
/// task-yield/block which are no-ops without an arbiter, and the
/// universal NEXT-modify F2 codes).
pub fn microengine_kernel(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    let mut o = Out::dont_care();

    // Decode the microinstruction.
    let mi: Microinstruction = unpack_kernel(i.instr);

    // Echo the MPC and current_task the owner gave us.
    o.mpc = i.mpc;
    o.current_task = i.current_task;

    // ---- BUS source -------------------------------------------------
    // BS = ReadR              → drive bus from R[rsel].
    // BS = MemoryData         → drive bus from memory[MAR] (1-cycle BRAM-delivered).
    // BS = InstructionRegister → drive bus from IR (Nova-emulator dispatch).
    // Other BS sources drive zero in Phase 3.5.
    let r_read: Bits<16> = q.regs.rdata;
    let bus_from_bs: Bits<16> = match mi.bs {
        BusSource::ReadR              => r_read,
        BusSource::MemoryData         => i.mem_read_data,
        BusSource::InstructionRegister => q.ir,
        _                             => bits::<16>(0),
    };
    // F1 = Constant overrides BUS with the constant-ROM lookup the
    // owner provided for this cycle's instruction.  Index = (RSEL << 3) | BS.
    let bus: Bits<16> = if mi.f1 == F1Function::Constant {
        i.constant_value
    } else {
        bus_from_bs
    };

    // ---- ALU -------------------------------------------------------
    // SKIP signal — Phase 1 doesn't have a cross-cycle skip latch;
    // wire it to the previous cycle's carry-out via L's MSB as a
    // placeholder (matches the Alto's "skip = previous-ALU-carry"
    // interpretation for the only Phase-1-relevant case).
    let skip: bool = (q.l & bits::<16>(0x8000)) != bits::<16>(0);
    let aout: AluOut = alu(mi.aluf, bus, q.t, skip);

    // ---- T and L latches ------------------------------------------
    // T_LOAD: T ← BUS (the manual: T-load "loads T from the bus
    // BEFORE the ALU computation".  For Phase 1 we treat it as
    // post-ALU; the only difference is the ALU sees the OLD T this
    // cycle either way.  To be revisited in Phase 2 with the spec.)
    d.t = if mi.t_load { bus } else { q.t };
    d.l = if mi.l_load { aout.result } else { q.l };

    // ---- R write --------------------------------------------------
    // Phase 1: R is written when BS = LoadR.  Real Alto uses certain
    // F1/F2 codes; for the simple subset, BS = LoadR is enough to
    // express R-load patterns in hand-written test microcode.
    let r_wen: bool = mi.bs == BusSource::LoadR;
    d.regs = RegIn {
        raddr: mi.rsel,
        waddr: mi.rsel,
        wdata: aout.result,
        wen:   r_wen,
    };

    // ---- L shifts (F1) --------------------------------------------
    // F1 = LeftShift1 / RightShift1 / LeftCycle8 modify L AFTER the
    // L_LOAD step.  Apply on top of the candidate `d.l` above.
    let l_after_f1: Bits<16> = match mi.f1 {
        F1Function::LeftShift1  => d.l << 1,
        F1Function::RightShift1 => d.l >> 1,
        F1Function::LeftCycle8  => (d.l << 8) | (d.l >> 8),
        _                       => d.l,
    };
    d.l = l_after_f1;

    // ---- Next MPC ------------------------------------------------
    // Default: take the NEXT field from the microinstruction.
    // F2 may modify low bits based on conditions:
    //   BusEqZero          — set bit 0 if bus == 0
    //   ShiftLessThanZero  — set bit 0 if d.l[15] == 0 (post-shift)
    //   ShiftEqZero        — set bit 0 if d.l == 0 (post-shift)
    //   BusToNext          — OR low bits of bus into NEXT (computed-go-to)
    //   AluCarryToNext     — set bit 0 if ALU carry-out
    let mut next_addr: Bits<10> = mi.next;
    let mut bit0: Bits<10> = next_addr & bits::<10>(0x1);
    bit0 = match mi.f2 {
        F2Function::BusEqZero          => if bus == bits::<16>(0)             { bit0 | bits::<10>(0x1) } else { bit0 },
        F2Function::ShiftLessThanZero  => if (l_after_f1 & bits::<16>(0x8000)) == bits::<16>(0) { bit0 | bits::<10>(0x1) } else { bit0 },
        F2Function::ShiftEqZero        => if l_after_f1 == bits::<16>(0)      { bit0 | bits::<10>(0x1) } else { bit0 },
        F2Function::AluCarryToNext     => if aout.carry                       { bit0 | bits::<10>(0x1) } else { bit0 },
        _                              => bit0,
    };
    let next_addr_or_bus: Bits<10> = match mi.f2 {
        F2Function::BusToNext => next_addr | (bus.resize() & bits::<10>(0x3FF)),
        _                     => next_addr,
    };
    next_addr = (next_addr_or_bus & bits::<10>(0x3FE)) | bit0;
    // F2=IDispatch (Emulator only): OR IR[7:0] into the low 8 bits of
    // NEXT.  This is the Nova-emulator instruction-dispatch path —
    // routes execution to the per-opcode microcode handler keyed by
    // the IR.  Real Alto IDISP also factors in the AC source ROM;
    // Phase 3.5 simplification: just OR IR[7:0] directly.
    let is_emulator_for_idisp: bool = i.current_task == bits::<4>(0);
    let is_idisp: bool = mi.f2 == F2Function::IDispatch;
    if is_emulator_for_idisp && is_idisp {
        let ir_low: Bits<10> = (q.ir & bits::<16>(0xFF)).resize();
        next_addr = next_addr | ir_low;
    }
    o.next_mpc = next_addr;

    // ---- DMA detection (Disk Word task) --------------------------
    // F2 = DiskWordTransfer + current_task == 14 (Disk Word) overrides
    // both the memory-bus and disk-ctrl outputs to perform an atomic
    // per-word DMA: memory[KCWA] ← disk_word_data; KCWA ← KCWA + 1.
    let is_disk_word_task: bool = i.current_task == bits::<4>(14);
    let is_dma: bool = is_disk_word_task && (mi.f2 == F2Function::DiskWordTransfer);

    // ---- Memory bus -----------------------------------------------
    // F2 = LoadMar → MAR ← BUS (commit at edge).
    // F2 = WriteMd → emit a memory write at q.mar with BUS as data.
    // DMA      → emit a memory write at i.kcwa with i.disk_word_data.
    // BS = MemoryData reads memory[MAR] (1-cycle delay on BRAM).
    d.mar = if mi.f2 == F2Function::LoadMar { bus } else { q.mar };
    o.mem_address    = if is_dma { i.kcwa } else { q.mar };
    o.mem_write_en   = is_dma || (mi.f2 == F2Function::WriteMd);
    o.mem_write_data = if is_dma { i.disk_word_data } else { bus };

    // ---- Per-task disk-controller register writes ---------------
    // Three F1 codes route BUS into specific disk-controller registers,
    // gated to current_task == 4 (Disk Sector):
    //   F1 = WriteKcomm → disk_ctrl_addr = REG_KCOM (2)
    //   F1 = WriteKadr  → disk_ctrl_addr = REG_KADR (3)
    //   F1 = WriteKdata → disk_ctrl_addr = REG_KDATA (1)
    // DMA path overrides: writes KCWA + 1 to REG_KCWA (4).
    let is_disk_sector_task: bool = i.current_task == bits::<4>(4);
    let is_kcomm: bool = mi.f1 == F1Function::WriteKcomm;
    let is_kadr:  bool = mi.f1 == F1Function::WriteKadr;
    let is_kdata: bool = mi.f1 == F1Function::WriteKdata;
    let is_kcwa:  bool = mi.f1 == F1Function::WriteKcwa;
    o.disk_ctrl_addr = if is_dma {
        bits::<3>(4)  // REG_KCWA
    } else if is_kcomm {
        bits::<3>(2)  // REG_KCOM
    } else if is_kadr {
        bits::<3>(3)  // REG_KADR
    } else if is_kdata {
        bits::<3>(1)  // REG_KDATA
    } else if is_kcwa {
        bits::<3>(4)  // REG_KCWA
    } else {
        bits::<3>(0)  // any value; write_en will be false
    };
    o.disk_ctrl_write_en   = is_dma || (is_disk_sector_task && (is_kcomm || is_kadr || is_kdata || is_kcwa));
    o.disk_ctrl_write_data = if is_dma { i.kcwa + bits::<16>(1) } else { bus };
    o.disk_word_consumed   = is_dma;
    o.task_yield           = mi.f1 == F1Function::TaskYield;

    // ---- Per-task IR load (Emulator) ------------------------------
    // F2 = LoadIr + current_task == 0 (Emulator) → IR ← MD
    // (memory data delivered from previous cycle's MAR).  In any
    // other task, this F2 code is a no-op.
    let is_emulator_task: bool = i.current_task == bits::<4>(0);
    let is_load_ir: bool = mi.f2 == F2Function::LoadIr;
    d.ir = if is_emulator_task && is_load_ir { i.mem_read_data } else { q.ir };
    o.ir = q.ir;

    // ---- Outputs ---------------------------------------------------
    o.t          = q.t;
    o.l          = q.l;
    o.bus        = bus;
    o.alu_result = aout.result;

    if cr.reset.any() {
        d.t = bits::<16>(0);
        d.l = bits::<16>(0);
        d.regs = RegIn {
            raddr: bits::<5>(0),
            waddr: bits::<5>(0),
            wdata: bits::<16>(0),
            wen: false,
        };
        d.mar = bits::<16>(0);
        d.ir = bits::<16>(0);
        o.mpc = bits::<10>(0);
        o.current_task = bits::<4>(0);
        o.next_mpc = bits::<10>(0);
        o.t = bits::<16>(0);
        o.l = bits::<16>(0);
        o.bus = bits::<16>(0);
        o.alu_result = bits::<16>(0);
        o.mem_address = bits::<16>(0);
        o.mem_write_en = false;
        o.mem_write_data = bits::<16>(0);
        o.disk_ctrl_addr = bits::<3>(0);
        o.disk_ctrl_write_en = false;
        o.disk_ctrl_write_data = bits::<16>(0);
        o.disk_word_consumed = false;
        o.task_yield = false;
        o.ir = bits::<16>(0);
    }
    (o, d)
}

#[kernel]
/// Inverse of [`Microinstruction::pack`] expressed as a kernel —
/// needed because the kernel can't call `Microinstruction::unpack`
/// (which is a non-kernel `impl` method).
pub fn unpack_kernel(word: Bits<32>) -> Microinstruction {
    let rsel: Bits<5>   = ((word >> 27) & bits::<32>(0x1F)).resize();
    let aluf_idx: Bits<4> = ((word >> 23) & bits::<32>(0xF)).resize();
    let bs_idx: Bits<3>   = ((word >> 20) & bits::<32>(0x7)).resize();
    let f1_idx: Bits<4>   = ((word >> 16) & bits::<32>(0xF)).resize();
    let f2_idx: Bits<4>   = ((word >> 12) & bits::<32>(0xF)).resize();
    let t_load: bool      = ((word >> 11) & bits::<32>(0x1)) != bits::<32>(0);
    let l_load: bool      = ((word >> 10) & bits::<32>(0x1)) != bits::<32>(0);
    let next: Bits<10>    = (word & bits::<32>(0x3FF)).resize();

    Microinstruction {
        rsel,
        aluf: aluf_from_index(aluf_idx),
        bs:   bs_from_index(bs_idx),
        f1:   f1_from_index(f1_idx),
        f2:   f2_from_index(f2_idx),
        t_load,
        l_load,
        next,
    }
}

#[kernel]
fn aluf_from_index(i: Bits<4>) -> AluFunction {
    if      i == bits::<4>(0)  { AluFunction::Bus }
    else if i == bits::<4>(1)  { AluFunction::T }
    else if i == bits::<4>(2)  { AluFunction::BusOrT }
    else if i == bits::<4>(3)  { AluFunction::BusAndT }
    else if i == bits::<4>(4)  { AluFunction::BusXorT }
    else if i == bits::<4>(5)  { AluFunction::BusPlusOne }
    else if i == bits::<4>(6)  { AluFunction::BusMinusOne }
    else if i == bits::<4>(7)  { AluFunction::BusPlusT }
    else if i == bits::<4>(8)  { AluFunction::BusMinusT }
    else if i == bits::<4>(9)  { AluFunction::BusMinusTMinusOne }
    else if i == bits::<4>(10) { AluFunction::BusPlusTPlusOne }
    else if i == bits::<4>(11) { AluFunction::BusPlusSkip }
    else if i == bits::<4>(12) { AluFunction::BusAndTAlt }
    else if i == bits::<4>(13) { AluFunction::BusAndNotT }
    else if i == bits::<4>(14) { AluFunction::Undef14 }
    else                       { AluFunction::Undef15 }
}

#[kernel]
fn bs_from_index(i: Bits<3>) -> BusSource {
    if      i == bits::<3>(0) { BusSource::ReadR }
    else if i == bits::<3>(1) { BusSource::LoadR }
    else if i == bits::<3>(2) { BusSource::None }
    else if i == bits::<3>(3) { BusSource::TaskSpec3 }
    else if i == bits::<3>(4) { BusSource::TaskSpec4 }
    else if i == bits::<3>(5) { BusSource::MemoryData }
    else if i == bits::<3>(6) { BusSource::Mouse }
    else                      { BusSource::InstructionRegister }
}

#[kernel]
fn f1_from_index(i: Bits<4>) -> F1Function {
    if      i == bits::<4>(0)  { F1Function::Nop }
    else if i == bits::<4>(1)  { F1Function::LeftShift1 }
    else if i == bits::<4>(2)  { F1Function::RightShift1 }
    else if i == bits::<4>(3)  { F1Function::LeftCycle8 }
    else if i == bits::<4>(4)  { F1Function::Constant }
    else if i == bits::<4>(5)  { F1Function::TaskYield }
    else if i == bits::<4>(6)  { F1Function::Block }
    else if i == bits::<4>(7)  { F1Function::Reserved7 }
    else if i == bits::<4>(8)  { F1Function::Reserved8 }
    else if i == bits::<4>(9)  { F1Function::Reserved9 }
    else if i == bits::<4>(10) { F1Function::Reserved10 }
    else if i == bits::<4>(11) { F1Function::Reserved11 }
    else if i == bits::<4>(12) { F1Function::WriteKcwa }
    else if i == bits::<4>(13) { F1Function::WriteKcomm }
    else if i == bits::<4>(14) { F1Function::WriteKadr }
    else                       { F1Function::WriteKdata }
}

#[kernel]
fn f2_from_index(i: Bits<4>) -> F2Function {
    if      i == bits::<4>(0)  { F2Function::Nop }
    else if i == bits::<4>(1)  { F2Function::BusEqZero }
    else if i == bits::<4>(2)  { F2Function::ShiftLessThanZero }
    else if i == bits::<4>(3)  { F2Function::ShiftEqZero }
    else if i == bits::<4>(4)  { F2Function::BusToNext }
    else if i == bits::<4>(5)  { F2Function::AluCarryToNext }
    else if i == bits::<4>(6)  { F2Function::LoadMar }
    else if i == bits::<4>(7)  { F2Function::WriteMd }
    else if i == bits::<4>(8)  { F2Function::DiskWordTransfer }
    else if i == bits::<4>(9)  { F2Function::Reserved9 }
    else if i == bits::<4>(10) { F2Function::Reserved10 }
    else if i == bits::<4>(11) { F2Function::Reserved11 }
    else if i == bits::<4>(12) { F2Function::LoadIr }
    else if i == bits::<4>(13) { F2Function::IDispatch }
    else if i == bits::<4>(14) { F2Function::Reserved14 }
    else                       { F2Function::Reserved15 }
}

/// Mark an unused-import suppressor.  `RegOut` is exposed in the
/// public API of `regfile` but isn't consumed inside this kernel
/// directly (we read `q.regs.rdata` via the auto-derived path);
/// keep the import for downstream documentation.
#[allow(dead_code)]
fn _force_use(_x: RegOut) {}
