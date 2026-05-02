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
// Note: the RegFile widget (in `regfile.rs`) is no longer composed
// as a sub-widget of Microengine — its DFF storage is inlined into
// Microengine directly per the Path B refactor.  RegFile is kept as
// a standalone widget for its own tests + as documentation of the
// regfile API surface.
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
    /// KSTAT register value (16-bit disk status).  Per *Alto Hardware
    /// Manual* §2.1 + spec §3.2 + §8.5: in Disk Sector / Disk Word
    /// task, BS=3 reads this onto the bus.
    pub kstat: Bits<16>,
    /// KDATA register value (16-bit disk data).  Per *Alto Hardware
    /// Manual* §2.1 + spec §3.2 + §8.5: in Disk Sector / Disk Word
    /// task, BS=4 reads this onto the bus.
    pub kdata: Bits<16>,
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
    /// True when this cycle's microinstruction has F1=Block (BLOCK).
    /// Per *Alto Hardware Manual* §2.4 + spec §5.5: BLOCK is "a
    /// hardware convention" by which the running task asks its
    /// associated **device interface** to deassert that device's
    /// wakeup signal.  The microengine surfaces the F1=Block bit;
    /// device widgets (e.g. DiabloDisk) snoop this in conjunction
    /// with `current_task` to decide whether to drop their wakeup.
    pub block_task: bool,
    /// True when current_task is a Disk task (4 or 14) AND
    /// F1=12 (CLRSTAT per spec §3.3 + §8.5).  Routed to the
    /// disk_controller to clear KSTAT (and any error latches once
    /// implemented).  KSEC at MPC=0x37c uses this as its first
    /// instruction.
    pub disk_clr_stat: bool,
    /// True when current_task is a Disk task AND F1=11B (binary 9,
    /// STROBE per spec §8.5): "Initiates a disk seek operation.
    /// KDATA must be loaded previously, and SENDADR bit of KCOM
    /// register set to 1."  Per §8.6, the disk hardware auto-streams
    /// sector words after position setup, so STROBE effectively
    /// initiates the read transfer for boot (sector 0/head 0 — no
    /// seek needed).  Routed to disk.transfer_request to arm the
    /// 256-word transfer.
    pub disk_strobe: bool,
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
///
/// **Path B refactor**: the R-register file's storage is now a DFF
/// owned directly by Microengine (not a sub-widget).  This collapses
/// the 1-cycle Q-register delay that sub-widget composition would
/// introduce, so the engine kernel can compute the **effective RSEL**
/// per-cycle (e.g., overridden by F2=ACSOURCE / F2=ACDEST in the
/// Emulator task per spec §6.6) and read R[effective_rsel]
/// combinationally in the same cycle.  Without this, ACSOURCE/ACDEST
/// would only address the IR-derived register one cycle later, which
/// is microcode-incompatible with real Alto.
#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct Microengine {
    /// T register (auxiliary ALU operand).
    t: dff::DFF<Bits<16>>,
    /// L register (latched ALU result).
    l: dff::DFF<Bits<16>>,
    /// R-register file storage (32 × 16 bits).  Inlined as a single
    /// DFF (per the §3.1 protocol-PHY pattern in CLAUDE.md) instead
    /// of the previous `RegFile` sub-widget.  Combinational reads
    /// happen in microengine_kernel via `q.regs[effective_rsel]`.
    regs: dff::DFF<[Bits<16>; 32]>,
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

    // ---- Effective RSEL (per spec §6.6 ACDEST/ACSOURCE) -----------
    // ACDEST (Emulator F2=11, binary 11): "(IR[3-4] XOR 3) to be
    // used as the low order two bits of the RSELECT field. This
    // addresses the accumulators from the destination field of the
    // instruction."
    // ACSOURCE (Emulator F2=14, binary 14): "(IR[1-2] XOR 3) ...
    // allowing the emulator to address its accumulators."
    //
    // Alto's MSB=0 numbering: IR[3-4] = our bits 12-11; IR[1-2] =
    // our bits 14-13.  Both override RSEL[1:0] (low 2 bits) with
    // the IR-derived AC index (XOR'd with 0b11), preserving RSEL[4:2]
    // from the microinstruction.
    //
    // (Note F2=11 = our F2Function::Code11; F2=14 = Code14.)
    let is_emulator: bool = i.current_task == bits::<4>(0);
    let ir_dest_ac: Bits<5> = (((q.ir >> 11) & bits::<16>(0b11))
        ^ bits::<16>(0b11)).resize();
    let ir_src_ac: Bits<5> = (((q.ir >> 13) & bits::<16>(0b11))
        ^ bits::<16>(0b11)).resize();
    let rsel_high: Bits<5> = mi.rsel & bits::<5>(0b11100);
    let effective_rsel: Bits<5> = if is_emulator
        && mi.f2 == F2Function::Code11
    {
        rsel_high | ir_dest_ac
    } else if is_emulator && mi.f2 == F2Function::Code14 {
        rsel_high | ir_src_ac
    } else {
        mi.rsel
    };

    // ---- BUS source -------------------------------------------------
    // BS = ReadR              → drive bus from R[effective_rsel].
    //   Reads the R-register file COMBINATIONALLY this cycle (Path B
    //   refactor: regfile DFF inlined into Microengine).  This is what
    //   ACSOURCE/ACDEST need — same-cycle effective_rsel → same-cycle
    //   regfile read, matching real Alto's 1-cycle pipeline.
    // BS = MemoryData         → drive bus from memory[MAR] (1-cycle BRAM-delivered).
    // BS = InstructionRegister → drive bus from IR (Nova-emulator dispatch).
    // Other BS sources drive zero in Phase 3.5.
    let r_read: Bits<16> = q.regs[effective_rsel];
    // Per *Alto Hardware Manual* §2.1 Bus Sources + spec §3.2 + §8.5,
    // BS=3 and BS=4 are TASK-SPECIFIC.  In Disk Sector (4) and Disk
    // Word (14) tasks, BS=3 reads KSTAT and BS=4 reads KDATA.  In
    // Emulator (0), BS=3 is ReadSLocation and BS=4 is LoadSLocation
    // (S-register access — not yet implemented; returns 0).
    let is_disk_task: bool = i.current_task == bits::<4>(4)
        || i.current_task == bits::<4>(14);
    let bus_from_bs: Bits<16> = match mi.bs {
        BusSource::ReadR              => r_read,
        BusSource::MemoryData         => i.mem_read_data,
        BusSource::InstructionRegister => q.ir,
        BusSource::TaskSpec3 => if is_disk_task { i.kstat } else { bits::<16>(0) },
        BusSource::TaskSpec4 => if is_disk_task { i.kdata } else { bits::<16>(0) },
        _                             => bits::<16>(0),
    };
    // F1 = Constant OR F2 = Constant overrides BUS with the constant-ROM
    // lookup the owner provided for this cycle's instruction.  Index =
    // (RSEL << 3) | BS.  Per ContrAlto's MicroInstruction.cs, both
    // F1 = 7 (SpecialFunction1.Constant) and F2 = 7
    // (SpecialFunction2.Constant) trigger the constant-ROM lookup.
    let bus: Bits<16> = if mi.f1 == F1Function::Constant
        || mi.f2 == F2Function::Constant {
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
    // express R-load patterns in hand-written test microcode.  Per
    // spec §6.6, ACDEST/ACSOURCE also affect the WRITE address (same
    // RSEL, since the regfile has a single addressed port for
    // both read and write).
    let r_wen: bool = mi.bs == BusSource::LoadR;
    let mut next_regs: [Bits<16>; 32] = q.regs;
    if r_wen {
        next_regs[effective_rsel] = aout.result;
    }
    d.regs = next_regs;

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
    // F2=IDispatch (Emulator only, F2=15B = binary 13): the 16-way
    // PROM dispatch per *Alto Hardware Manual* §3.5 + spec §6.6.
    // The actual table from spec §6.6:
    //
    //   Conditions          | OR'd onto NEXT
    //   --------------------|-----------------
    //   if IR[1-2] = 0      | IR[3-4]
    //   elseif IR[1-2] = 1  | 4
    //   elseif IR[1-2] = 2  | 5
    //   elseif IR[4-7] = 0  | 1
    //   elseif IR[4-7] = 1  | 0
    //   elseif IR[4-7] = 6  | 16B (= 14)
    //   else                | IR[4-7]
    //
    // Alto's MSB=0 numbering: IR[1-2] = our bits 14-13 (top two below
    // the MSB); IR[3-4] = our bits 12-11; IR[4-7] = our bits 11-8.
    let is_emulator_for_idisp: bool = i.current_task == bits::<4>(0);
    let is_idisp: bool = mi.f2 == F2Function::IDispatch;
    if is_emulator_for_idisp && is_idisp {
        let ir_1_2: Bits<10> = ((q.ir >> 13) & bits::<16>(0b11)).resize();
        let ir_3_4: Bits<10> = ((q.ir >> 11) & bits::<16>(0b11)).resize();
        let ir_4_7: Bits<10> = ((q.ir >> 8)  & bits::<16>(0b1111)).resize();
        let dispatch_value: Bits<10> =
                 if ir_1_2 == bits::<10>(0) { ir_3_4 }
            else if ir_1_2 == bits::<10>(1) { bits::<10>(4) }
            else if ir_1_2 == bits::<10>(2) { bits::<10>(5) }
            else if ir_4_7 == bits::<10>(0) { bits::<10>(1) }
            else if ir_4_7 == bits::<10>(1) { bits::<10>(0) }
            else if ir_4_7 == bits::<10>(6) { bits::<10>(0o16) }
            else                            { ir_4_7 };
        next_addr = next_addr | dispatch_value;
    }

    // Per-task F2 NEXT-modify codes for Disk tasks per *Alto Hardware
    // Manual* §6.0 + spec §8.5.  All apply only when current_task is
    // a Disk task (4 = Disk Sector, 14 = Disk Word).
    //
    //   F2=10B (binary 8) INIT — NEXT or= (37B if WDTASKACT&WDINIT, else 0)
    //   F2=11B (binary 9) RWC  — NEXT or= (3 write / 2 check / 0 read)
    //   F2=12B (binary 10) RECNO — NEXT or= MAP(record-number)
    //                              MAP(0)=0, MAP(1)=2, MAP(2)=3, MAP(3)=1
    //   F2=13B (binary 11) XFRDAT — NEXT or= (1 if data transfer, else 0)
    //   F2=14B (binary 12) SWRNRDY — NEXT or= (1 if disk not ready, else 0)
    //   F2=15B (binary 13) NFER — NEXT or= (0 if fatal error, else 1)
    //   F2=16B (binary 14) STROBON — NEXT or= (1 if seek strobe still on, else 0)
    //
    // Phase-3.5 stub values (no full disk-state model):
    //   WDTASKACT/WDINIT — false (initial state passed; KSEC's first
    //                      cycle observes false → no NEXT mod).
    //   RWC — read (0).  Boot reads sector data; matches.
    //   RECNO — 0 (header).  Boot starts with header record; matches
    //                        spec §8.6 KBLK chain.
    //   XFRDAT — 0 (no transfer in progress on most cycles; only
    //              meaningful during active DMA which we collapse).
    //   SWRNRDY — 0 (always ready in our sim).
    //   NFER — 1 (no errors modeled — set NEXT bit 0 to indicate
    //          "no fatal error" path).  This is the only stub that
    //          actually modifies NEXT in steady state.
    //   STROBON — 0 (instant seek; no in-progress strobe).
    //
    // Each F2 NEXT-modify is OR'd into next_addr; multiple can apply
    // if the dispatch is ambiguous, but the canonical KSEC/KWDX
    // microcode uses one per cycle.
    let in_disk_task_for_f2: bool = i.current_task == bits::<4>(4)
        || i.current_task == bits::<4>(14);
    if in_disk_task_for_f2 {
        // INIT, XFRDAT, RWC, RECNO, SWRNRDY, STROBON all stub to 0
        // (no NEXT modification).  NFER stubs to 1 (always set bit 0).
        // F2 = IDispatch (binary 13) in Disk task = NFER per spec
        // §8.5 — distinct from Emulator's IDISP semantics handled
        // above (the Emulator-vs-Disk dispatch is gated by
        // current_task, so the two never collide).
        if mi.f2 == F2Function::IDispatch {
            next_addr = next_addr | bits::<10>(1);
        }
        // Other F2 disk codes: stub 0 → no modification → no-op.
        // F2=DiskWordTransfer (= INIT in spec §8.5) gets the existing
        // DMA atomic dispatch in Disk Word task, plus NEXT-modify
        // semantics fall through to the WDTASKACT&WDINIT case which
        // is false in steady state.
    }

    // F2=BUSODD (Emulator-only, F2=10B/binary 8 per spec §6.6):
    // "BUSODD merges BUS[15] into NEXT[9]".  Alto's MSB=0 numbering
    // means BUS[15] = LSB (our bit 0) and NEXT[9] = LSB of the
    // 10-bit NEXT (our bit 0).  So: if BUS bit 0 is set, set NEXT
    // bit 0.  (F2=DiskWordTransfer is the same binary code 8 — in
    // Disk task it's our atomic-DMA simplification + INIT NEXT-
    // modify; in Emulator it's BUSODD.  Per-task dispatch
    // distinguishes.)
    let is_emulator_for_busodd: bool = i.current_task == bits::<4>(0);
    let is_busodd: bool = mi.f2 == F2Function::DiskWordTransfer;
    if is_emulator_for_busodd && is_busodd {
        let bus_lsb: Bits<10> = (bus & bits::<16>(1)).resize();
        next_addr = next_addr | bus_lsb;
    }
    o.next_mpc = next_addr;

    // ---- DMA detection (Disk Word task) --------------------------
    // F2 = DiskWordTransfer + current_task == 14 (Disk Word) overrides
    // both the memory-bus and disk-ctrl outputs to perform an atomic
    // per-word DMA: memory[KCWA] ← disk_word_data; KCWA ← KCWA + 1.
    let is_disk_word_task: bool = i.current_task == bits::<4>(14);
    let is_dma: bool = is_disk_word_task && (mi.f2 == F2Function::DiskWordTransfer);

    // ---- Memory bus -----------------------------------------------
    // F1 = LoadMar (universal, F1 = 1) → MAR ← BUS (commit at edge).
    // F2 = StoreMd (universal, F2 = 6) → emit a memory write at q.mar
    //                                     with BUS as data.
    // DMA → emit a memory write at i.kcwa with i.disk_word_data.
    // BS = MemoryData reads memory[MAR] (1-cycle delay on BRAM).
    d.mar = if mi.f1 == F1Function::LoadMar { bus } else { q.mar };
    o.mem_address    = if is_dma { i.kcwa } else { q.mar };
    o.mem_write_en   = is_dma || (mi.f2 == F2Function::StoreMd);
    o.mem_write_data = if is_dma { i.disk_word_data } else { bus };

    // ---- Per-task disk-controller register writes ---------------
    // Per *Alto Hardware Manual* §2.1 + spec §3.3 + §8.5, F1 codes
    // 8-15 are task-specific.  In Disk Sector (4) and Disk Word (14)
    // tasks the writable disk-controller register paths are:
    //   F1 = 10 (Code10) = LoadKSTAT → KSTAT  (REG_KSTAT = 0)
    //   F1 = 13 (WriteKcomm) = LoadKCOMM → KCOM (REG_KCOM = 2)
    //   F1 = 14 (WriteKadr)  = LoadKADR  → KADR (REG_KADR = 3)
    //   F1 = 15 (WriteKdata) = LoadKDATA → KDATA (REG_KDATA = 1)
    // F1 = 12 (WriteKcwa) is our Phase-3.5 simplification — real Alto
    // F1=12 is CLRSTAT.  Kept for the legacy DMA test microcode that
    // sets KCWA via this F1 code.  Real microcode should not hit it.
    // DMA path overrides: writes KCWA + 1 to REG_KCWA (4).
    let is_disk_sector_task: bool = i.current_task == bits::<4>(4);
    let is_disk_word_task_for_f1: bool = i.current_task == bits::<4>(14);
    let in_disk_task: bool = is_disk_sector_task || is_disk_word_task_for_f1;
    let is_kcomm: bool = mi.f1 == F1Function::WriteKcomm;
    let is_kadr:  bool = mi.f1 == F1Function::WriteKadr;
    let is_kdata: bool = mi.f1 == F1Function::WriteKdata;
    let is_kcwa:  bool = mi.f1 == F1Function::WriteKcwa;
    let is_kstat: bool = mi.f1 == F1Function::Code10;
    o.disk_ctrl_addr = if is_dma {
        bits::<3>(4)  // REG_KCWA
    } else if is_kstat {
        bits::<3>(0)  // REG_KSTAT
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
    o.disk_ctrl_write_en   = is_dma
        || (is_disk_sector_task && (is_kcomm || is_kadr || is_kdata || is_kcwa))
        || (in_disk_task && is_kstat);
    o.disk_ctrl_write_data = if is_dma { i.kcwa + bits::<16>(1) } else { bus };
    o.disk_word_consumed   = is_dma;
    o.task_yield           = mi.f1 == F1Function::TaskYield;
    // F1=Block (universal, F1=3): per *Alto Hardware Manual* §2.4
    // and spec §5.5.  Surfaced as a chip-level signal so device
    // widgets (e.g. DiabloDisk) can snoop it together with
    // current_task to deassert their own wakeup signals.
    o.block_task           = mi.f1 == F1Function::Block;
    // F1=STROBE (per-task, F1=11B/binary 9 in Disk Sector / Disk
    // Word per spec §8.5): "Initiates a disk seek operation. KDATA
    // must be loaded previously, and SENDADR bit of KCOM register
    // set to 1."  Per §8.6, the disk hardware auto-streams sector
    // words after position setup, so for boot (sector 0/head 0/no
    // seek needed), STROBE effectively also initiates the read
    // transfer.  Surfaced as `disk_strobe` for the chip to route to
    // disk.transfer_request as an arm trigger (in addition to the
    // existing Phase-3.5 KCOM-bit-15 path).
    o.disk_strobe = (i.current_task == bits::<4>(4)
        || i.current_task == bits::<4>(14))
        && (mi.f1 == F1Function::Code9);
    // F1=CLRSTAT (per-task, F1=12 in Disk Sector / Disk Word per
    // spec §3.3 + §8.5): "Causes all error latches in disk controller
    // hardware to reset, clears KSTAT[13]" (per ContrAlto's
    // DiskController.cs ClearStatus()).  Surfaced as `disk_clr_stat`
    // for the chip to route to disk_ctrl.  Note: F1=WriteKcwa is the
    // same binary code (12) — it serves both meanings for now: real
    // Alto microcode triggers CLRSTAT, the legacy DMA test microcode
    // triggers KCWA write.  The two semantics are not mutually
    // exclusive (clear KSTAT + write KCWA both happen in this
    // simplification).
    o.disk_clr_stat = (i.current_task == bits::<4>(4)
        || i.current_task == bits::<4>(14))
        && (mi.f1 == F1Function::WriteKcwa);

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
        d.regs = [bits::<16>(0); 32];
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
        o.block_task = false;
        o.disk_clr_stat = false;
        o.disk_strobe = false;
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
    else if i == bits::<4>(1)  { F1Function::LoadMar }
    else if i == bits::<4>(2)  { F1Function::TaskYield }
    else if i == bits::<4>(3)  { F1Function::Block }
    else if i == bits::<4>(4)  { F1Function::LeftShift1 }
    else if i == bits::<4>(5)  { F1Function::RightShift1 }
    else if i == bits::<4>(6)  { F1Function::LeftCycle8 }
    else if i == bits::<4>(7)  { F1Function::Constant }
    else if i == bits::<4>(8)  { F1Function::EmuSwMode }
    else if i == bits::<4>(9)  { F1Function::Code9 }
    else if i == bits::<4>(10) { F1Function::Code10 }
    else if i == bits::<4>(11) { F1Function::Code11 }
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
    else if i == bits::<4>(6)  { F2Function::StoreMd }
    else if i == bits::<4>(7)  { F2Function::Constant }
    else if i == bits::<4>(8)  { F2Function::DiskWordTransfer }
    else if i == bits::<4>(9)  { F2Function::Code9 }
    else if i == bits::<4>(10) { F2Function::Code10 }
    else if i == bits::<4>(11) { F2Function::Code11 }
    else if i == bits::<4>(12) { F2Function::LoadIr }
    else if i == bits::<4>(13) { F2Function::IDispatch }
    else if i == bits::<4>(14) { F2Function::Code14 }
    else                       { F2Function::Code15 }
}

