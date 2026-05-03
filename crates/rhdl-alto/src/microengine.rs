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
    /// Memory-pipeline stall signal from the [`crate::memory::Memory`]
    /// subsystem (combinational this cycle).  When asserted, the
    /// microengine MUST freeze for this cycle:
    ///
    ///   - `o.next_mpc = i.mpc` (MPC doesn't advance — owner won't
    ///     update task_mpc).
    ///   - All internal DFFs (T, L, R-writes, MAR, IR, alu_carry, skip,
    ///     carry, next_modifier_pending) hold their current values.
    ///   - `o.task_yield = false` (no task switches during a stall).
    ///   - Memory write_en + DMA outputs hold (write side-effects to
    ///     the BRAM are still issued via `i.address` / `i.write_en`,
    ///     idempotent since `i.address` is also held by the engine
    ///     across the stall).
    ///
    /// Per AltoHW §2.3: "the processor will suspend execution of
    /// microinstructions if an `←MD` or `MD←` is executed before the
    /// memory interface is prepared to deliver or accept data."
    pub mem_stall: bool,
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
    /// True when current_task is Emulator (0) AND F1=17B (binary 15,
    /// STARTF per spec §6.6): "The STARTF function is used by the
    /// SIO instruction, and is used to define commands for I/O
    /// hardware, including the Ethernet."  Surfaced for the chip to
    /// route to per-device I/O triggers (boot disk read, Ethernet
    /// dispatch, etc.).  Same binary code as Disk task's WriteKdata
    /// (LoadKDATA per spec §8.5); per-task gating distinguishes.
    pub startf: bool,
    /// Instruction Register — current Nova instruction (Emulator task).
    pub ir: Bits<16>,
    /// R-register file (32 × 16 bits) — exposed for cycle-by-cycle
    /// register-state lockstep against ContrAlto.  Echoed from
    /// `q.regs` so consumers see the START-of-cycle (latched) values.
    pub regs: [Bits<16>; 32],
    /// True when this cycle's microinstruction has F1=LoadMar (= MAR<-).
    /// Routed by AltoChip to the Memory subsystem's pipeline-stall FSM
    /// to (a) reset cycles_since_mar to 1 and (b) detect "back-to-back
    /// MAR<- too soon" stall conditions.  Pure decode of the current
    /// uinst — visible to the chip without depending on stall state.
    pub mar_load_this_cycle: bool,
    /// True when this cycle's microinstruction has BS=MemoryData
    /// (= ←MD).  Routed by AltoChip to the Memory subsystem to detect
    /// "←MD too soon after MAR<-" stalls.
    pub md_read_this_cycle: bool,
    /// True when this cycle's microinstruction has F2=StoreMd (= MD<-).
    /// Routed by AltoChip to the Memory subsystem to detect "MD<- too
    /// soon after MAR<-" stalls.  Note: distinct from `mem_write_en`
    /// which also fires for DMA from Disk Word task; only the
    /// microcode-issued MD<- triggers the pipeline-stall FSM.
    pub md_write_this_cycle: bool,
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
    /// Sticky-from-last-L-load ALU carry, per spec §3.4 footnote:
    /// "the carry [used by F2=ALUCY] is that produced by the ALU
    /// function which last loaded the L register."  Updated whenever
    /// L_LOAD fires (with the current ALU's carry-out); read by
    /// F2=AluCarryToNext.
    alu_carry: dff::DFF<bool>,
    /// SKIP latch (Emulator only, per spec §6.6 / "NOVEL SHIFTS"
    /// commentary in ContrAlto's Shifter.cs).  Set by F2=LoadDNS based
    /// on Nova SKIP-condition (IR low 3 bits).  Cleared by F2=LoadIr.
    /// Read by ALUF=11 (BUS+SKIP): when SKIP is set, the ALU adds 1 to
    /// BUS instead of passing it through, implementing Nova's skip-on-
    /// condition (the next macroinstruction's PC increment).
    skip: dff::DFF<bool>,
    /// Nova CARRY flip-flop (Emulator only).  Used by F2=LoadDNS for
    /// Nova-style 17-bit rotates and carry-condition computations.
    /// Cleared by reset; updated by LoadDNS late phase based on the
    /// shifter result and the Nova arithmetic op's carry semantics.
    carry: dff::DFF<bool>,
    /// Pending F2 NEXT-modifier from the PREVIOUS cycle, OR'd into
    /// THIS cycle's NEXT field.  Per spec digest §2.3 ("F2 NEXT-
    /// modifier pipeline timing — DELAYED by one cycle"), F2
    /// modifications are latched at the end of cycle K and applied
    /// to cycle K+1's NEXT.  This is the AltoHW disambiguation
    /// established by the standard microcode behavior + ContrAlto's
    /// `Tasks/Task.cs:173,549`.  Cleared by reset.
    next_modifier_pending: dff::DFF<Bits<10>>,
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
    // F2=Code10 (LoadDNS) ALSO uses the destination-AC override, per
    // ContrAlto's EmulatorTask.cs early LoadDNS handler:
    // `_rSelect = (_rSelect & 0xfffc) | (((ir & 0x1800) >> 11) ^ 3)` —
    // identical to ACDEST.
    let effective_rsel: Bits<5> = if is_emulator
        && (mi.f2 == F2Function::Code11 || mi.f2 == F2Function::Code10)
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
    // BS = InstructionRegister (= spec name `←DISP`, the displacement
    // field of IR, NOT the full IR).  Per spec §3.2 + ContrAlto's
    // Task.cs ReadDisp handler:
    //   BUS = IR & 0xFF (low 8 bits = DISP field).
    //   IF X-field (IR bits 6-7 in Alto MSB=0 = our bits 9-8) != 0
    //      AND sign bit (IR bit 8 in Alto = our bit 7) is set:
    //      sign-extend by setting BUS bits 8-15.
    // Used by Nova memory-reference instructions to compute effective
    // address from the 8-bit displacement field.
    // Other BS sources drive zero in Phase 3.5.
    let r_read: Bits<16> = q.regs[effective_rsel];
    // Per *Alto Hardware Manual* §2.1 Bus Sources + spec §3.2 + §8.5,
    // BS=3 and BS=4 are TASK-SPECIFIC.  In Disk Sector (4) and Disk
    // Word (14) tasks, BS=3 reads KSTAT and BS=4 reads KDATA.  In
    // Emulator (0), BS=3 is ReadSLocation and BS=4 is LoadSLocation
    // (S-register access — not yet implemented; returns 0).
    let is_disk_task: bool = i.current_task == bits::<4>(4)
        || i.current_task == bits::<4>(14);
    // Bus source dispatch.  Per spec §3.2:
    //   - BusSource::None (BS=2) reads as -1 (all-ones) because
    //     no source is asserting and the Alto bus is wired-AND.
    //   - BusSource::LoadR (BS=1) forces BUS=0 AND triggers R-write.
    //   - TaskSpec3/4 in disk tasks read KSTAT/KDATA; in Emulator
    //     they're S-register access (not yet implemented; stub 0).
    //   - Mouse (BS=6) is not implemented; stub 0.
    let bus_from_bs: Bits<16> = match mi.bs {
        BusSource::ReadR              => r_read,
        BusSource::LoadR              => bits::<16>(0),
        BusSource::None               => bits::<16>(0xFFFF),
        BusSource::TaskSpec3 => if is_disk_task { i.kstat } else { bits::<16>(0) },
        BusSource::TaskSpec4 => if is_disk_task { i.kdata } else { bits::<16>(0) },
        BusSource::MemoryData         => i.mem_read_data,
        BusSource::Mouse              => bits::<16>(0),
        BusSource::InstructionRegister => {
            let disp: Bits<16> = q.ir & bits::<16>(0x00FF);
            let x_nonzero: bool = (q.ir & bits::<16>(0x0300)) != bits::<16>(0);
            let sign_bit_set: bool = (q.ir & bits::<16>(0x0080)) != bits::<16>(0);
            if x_nonzero && sign_bit_set {
                disp | bits::<16>(0xFF00)
            } else {
                disp
            }
        }
    };
    // F1 = Constant OR F2 = Constant overrides BUS with the constant-ROM
    // lookup the owner provided for this cycle's instruction.  Index =
    // (RSEL << 3) | BS.  Per ContrAlto's MicroInstruction.cs, both
    // F1 = 7 (SpecialFunction1.Constant) and F2 = 7
    // (SpecialFunction2.Constant) trigger the constant-ROM lookup.
    //
    // Per AltoHW §2.2 + spec digest §3.2, the constant ROM is ALSO
    // gated when BS ≥ 4, AND'd with the BS source (the wired-AND
    // masking facility for ←MOUSE / ←DISP / ←MD / TaskSpec4).
    // This is NOT yet implemented — see CHANGELOG entry "BS≥4 AND-
    // masking deferred" and follow-up tracker.  Most real-microcode
    // mask values are 0xFFFF (= no-op AND), so this is a quiet
    // correctness gap rather than an active source of divergence;
    // when a microinstruction hits a (RSEL, BS≥4) index with a
    // non-0xFFFF mask, the BUS value will be wrong.
    let bus: Bits<16> = if mi.f1 == F1Function::Constant
        || mi.f2 == F2Function::Constant {
        i.constant_value
    } else {
        bus_from_bs
    };

    // ---- ALU -------------------------------------------------------
    // SKIP signal — proper SKIP latch (replaces the Phase 1 placeholder
    // that read L's MSB).  Read by ALUF=11 (BUS+SKIP) per spec §3.1.
    // The latch is cleared by F2=LoadIr per spec §6.6 ("IR← clears
    // SKIP") and would be set by F2=LoadDNS (D16, not yet implemented).
    let skip: bool = q.skip;
    let aout: AluOut = alu(mi.aluf, bus, q.t, skip);

    // ---- T and L latches ------------------------------------------
    // T_LOAD: T ← BUS or ALU output, per spec §3.1 footnote: "If T is
    // loaded during an instruction which specifies [an asterisked ALU
    // function], it will be loaded from the ALU output rather than from
    // the bus."  Asterisked ALUFs are 2 (BusOrT), 5 (BusPlusOne),
    // 6 (BusMinusOne), 10 (BusPlusTPlusOne), 12 (BusAndTAlt).
    //
    // This is what makes accumulator patterns like `T← BUS OR T` work:
    // BUS drives the operand; ALU computes BUS|T; T_LOAD is set; T
    // receives the OR result, not just BUS.
    let aluf_t_loads_alu: bool =
           mi.aluf == AluFunction::BusOrT
        || mi.aluf == AluFunction::BusPlusOne
        || mi.aluf == AluFunction::BusMinusOne
        || mi.aluf == AluFunction::BusPlusTPlusOne
        || mi.aluf == AluFunction::BusAndTAlt;
    let t_source: Bits<16> = if aluf_t_loads_alu { aout.result } else { bus };
    d.t = if mi.t_load { t_source } else { q.t };
    d.l = if mi.l_load { aout.result } else { q.l };
    // Sticky carry: latch this ALU's carry only when L is loaded.
    // F2=AluCarryToNext (ALUCY) reads this register, NOT the current
    // ALU's carry, per spec §3.4 footnote.
    d.alu_carry = if mi.l_load { aout.carry } else { q.alu_carry };

    // ---- L shifts (F1) --------------------------------------------
    // F1 = LeftShift1 / RightShift1 / LeftCycle8 modify L AFTER the
    // L_LOAD step.  Apply on top of the candidate `d.l` above.  The
    // post-shift value is the **Shifter Output** per *Alto Hardware
    // Manual* §2.7 + ContrAlto's Task.cs (`Shifter.Output`) — and
    // it's what feeds the R-write path below.
    //
    // F2=MAGIC (Emulator-only, F2=9) modifies LSH/RSH to inject T bits
    // for double-length shifts per spec §6.6 + ContrAlto's Shifter.cs:
    //   - Left shift:  bit 0  ← T bit 15 (MSB of T)
    //   - Right shift: bit 15 ← T bit 0  (LSB of T)
    //   - LCY8: no MAGIC variant (per ContrAlto Shifter)
    //
    // F2=LoadDNS (Emulator-only, F2=10) modifies LSH/RSH to do Nova-
    // style 17-bit rotates with the dns_carry value (computed from
    // q.carry + IR bits 4-5 + Nova arith op + ALU C0):
    //   - Left shift:  bit 0  ← dns_carry; new carry ← input bit 15
    //   - Right shift: bit 15 ← dns_carry; new carry ← input bit 0
    let is_magic: bool = is_emulator && mi.f2 == F2Function::Code9;
    let is_dns:   bool = is_emulator && mi.f2 == F2Function::Code10;

    // Nova carry input for DNS, per ContrAlto's LoadDNS early handler.
    // IR bits 5-4 (in our LSB=0 numbering, value 0..3) select carry
    // base; then if Nova arith op (IR bits 10-8, value 0..7) is one of
    // NEG/INC/ADC/SUB/ADD AND ALU produced a carry-out, invert.
    let dns_carry_base: bool = if is_dns {
        let cc: Bits<2> = ((q.ir >> 4) & bits::<16>(0b11)).resize();
        if cc == bits::<2>(0)      { q.carry }
        else if cc == bits::<2>(1) { false }            // Z
        else if cc == bits::<2>(2) { true }             // O
        else                        { !q.carry }        // C: complement
    } else {
        q.carry
    };
    let dns_carry_in: bool = if is_dns {
        let op: Bits<3> = ((q.ir >> 8) & bits::<16>(0b111)).resize();
        let invert: bool =
               op == bits::<3>(1)
            || op == bits::<3>(3)
            || op == bits::<3>(4)
            || op == bits::<3>(5)
            || op == bits::<3>(6);
        if invert && aout.carry { !dns_carry_base } else { dns_carry_base }
    } else {
        false  // unused when !is_dns
    };

    // Capture pre-shift value (= L after L_LOAD but before F1's shift).
    // Needed so dns_carry_out can read the bit that gets rotated out.
    let l_pre_shift: Bits<16> = d.l;

    // Compute shifter output.
    let l_after_f1: Bits<16> = match mi.f1 {
        F1Function::LeftShift1  => {
            let shifted = l_pre_shift << 1;
            if is_magic        { shifted | ((q.t >> 15) & bits::<16>(1)) }
            else if is_dns     { shifted | (if dns_carry_in { bits::<16>(1) } else { bits::<16>(0) }) }
            else               { shifted }
        }
        F1Function::RightShift1 => {
            let shifted = l_pre_shift >> 1;
            if is_magic        { shifted | ((q.t & bits::<16>(1)) << 15) }
            else if is_dns     { shifted | (if dns_carry_in { bits::<16>(0x8000) } else { bits::<16>(0) }) }
            else               { shifted }
        }
        F1Function::LeftCycle8  => (l_pre_shift << 8) | (l_pre_shift >> 8),
        _                       => l_pre_shift,
    };
    d.l = l_after_f1;

    // dns_carry_out: the bit that gets rotated INTO the new carry.
    // Per ContrAlto: left shift → input bit 15; right shift → input bit 0.
    // For non-shift (e.g. just F2=LoadDNS without LSH/RSH), dns_carry_in.
    let dns_carry_out: bool = if is_dns {
        match mi.f1 {
            F1Function::LeftShift1  => (l_pre_shift & bits::<16>(0x8000)) != bits::<16>(0),
            F1Function::RightShift1 => (l_pre_shift & bits::<16>(1))      != bits::<16>(0),
            _                       => dns_carry_in,
        }
    } else {
        false
    };

    // ---- R write --------------------------------------------------
    // Per *Alto Hardware Manual* §2.7 + ContrAlto's `Task.cs`:
    //
    //   if (_loadR) { _cpu._r[_rSelect] = Shifter.Output; }
    //
    // R is loaded from the **Shifter Output** (= L with F1's shift
    // applied), NOT from the ALU result.  Our pre-fix bug wrote
    // aout.result instead, which is why microcode like `SAD_ L`
    // (intended R[5] ← L) was actually doing R[5] ← ALU = 0.  That
    // broke the boot dance Q-loop because SAD never advanced from 0
    // → MAR = SAD+T stayed = T → memory read/write degenerate →
    // IR stayed at 0 → dispatch stayed at Q0.
    //
    // Per spec §6.6, ACDEST/ACSOURCE also affect the WRITE address
    // (same RSEL, since the regfile has a single addressed port).
    // F2=LoadDNS suppresses R-write when IR[12] is set (Alto bit 12 =
    // our bit 3, the Nova "no-load" bit on shift instructions).  Per
    // ContrAlto: `_loadR = (_cpu._ir & 0x0008) == 0`.
    let dns_suppress_r: bool = is_dns && (q.ir & bits::<16>(0x0008)) != bits::<16>(0);
    let r_wen: bool = mi.bs == BusSource::LoadR && !dns_suppress_r;
    let mut next_regs: [Bits<16>; 32] = q.regs;
    if r_wen {
        next_regs[effective_rsel] = l_after_f1;
    }
    d.regs = next_regs;

    // ---- Next MPC + F2 NEXT-modifier (DELAYED pipeline) ---------
    //
    // Per spec digest §2.3 ("F2 NEXT-modifier pipeline timing —
    // DELAYED by one cycle"), F2 contributions to NEXT are latched
    // at the end of cycle K and applied to cycle K+1's NEXT field.
    // This matches the standard microcode (boot wait-loop iterates
    // 0x131, 0x132, 0x133 — only works with delayed semantics) and
    // ContrAlto's `Tasks/Task.cs:173-174 + 549`.
    //
    // Implementation pattern:
    //   1. Compute THIS cycle's F2 modifier `next_modifier_this_cycle`
    //      (the OR-bits to apply to the NEXT cycle's NEXT field).
    //   2. Set `next_addr = mi.next | q.next_modifier_pending`
    //      (use LAST cycle's latched modifier).
    //   3. Latch `d.next_modifier_pending = next_modifier_this_cycle`.
    //
    // F2 codes that contribute modifier bits (all OR'd, never replace):
    //   BusEqZero          — bit 0 if bus == 0
    //   ShiftLessThanZero  — bit 0 if l_after_f1[15] == 1
    //   ShiftEqZero        — bit 0 if l_after_f1 == 0
    //   AluCarryToNext     — bit 0 from sticky alu_carry (last L-load)
    //   BusToNext          — bus low 10 bits
    //   IDispatch (Emul)   — 4-bit PROM dispatch (16-way)
    //   IDispatch (Disk)   — bit 0 (NFER stub: no-error path)
    //   ACSOURCE (Emul)    — IR-derived dispatch + indirect bit
    //   LoadIr (Emul)      — bus bits 0,5,6,7 merged into NEXT[0-3]
    //   BUSODD (Emul)      — bus bit 0 → NEXT bit 0
    let mut next_modifier_this_cycle: Bits<10> = bits::<10>(0);
    next_modifier_this_cycle = next_modifier_this_cycle | match mi.f2 {
        F2Function::BusEqZero          => if bus == bits::<16>(0)             { bits::<10>(0x1) } else { bits::<10>(0) },
        // Spec §3.4: SH<0 sets NEXT bit 0 if Shifter Output is negative
        // (MSB SET).  Previous code had this inverted (== 0 instead of != 0),
        // making the engine branch the wrong way on signed comparisons.
        F2Function::ShiftLessThanZero  => if (l_after_f1 & bits::<16>(0x8000)) != bits::<16>(0) { bits::<10>(0x1) } else { bits::<10>(0) },
        F2Function::ShiftEqZero        => if l_after_f1 == bits::<16>(0)      { bits::<10>(0x1) } else { bits::<10>(0) },
        // ALUCY uses the STICKY carry from the last cycle that loaded L,
        // NOT this cycle's carry.  Per spec §3.4 footnote.
        F2Function::AluCarryToNext     => if q.alu_carry                      { bits::<10>(0x1) } else { bits::<10>(0) },
        F2Function::BusToNext          => bus.resize() & bits::<10>(0x3FF),
        _                              => bits::<10>(0),
    };
    // F2=IDispatch (Emulator only, F2=15B = binary 13): the 16-way
    // PROM dispatch per *Alto Hardware Manual* §3.5 + spec §6.6.
    // Full table per ContrAlto's EmulatorTask.cs (verified against
    // the spec — D12 fix added the IR[0]=1 branch and the
    // IR[4-7]=16B branch which were missing):
    //
    //   Conditions          | OR'd onto NEXT          | Comment
    //   --------------------|-----------------        |--------
    //   if IR[0] = 1        | 3 - IR[8-9]             | complement of SH field (Nova SHIFT)
    //   elseif IR[1-2] = 0  | IR[3-4]                 | JMP, JSR, ISZ, DSZ
    //   elseif IR[1-2] = 1  | 4                       | LDA
    //   elseif IR[1-2] = 2  | 5                       | STA
    //   elseif IR[4-7] = 0  | 1                       |
    //   elseif IR[4-7] = 1  | 0                       |
    //   elseif IR[4-7] = 6  | 16B (= 14)              | CONVERT
    //   elseif IR[4-7] = 16B(=14) | 6                 |
    //   else                | IR[4-7]                 |
    //
    // Alto's MSB=0 numbering:
    //   IR[0]    = our bit 15 (MSB)
    //   IR[1-2]  = our bits 14-13
    //   IR[3-4]  = our bits 12-11
    //   IR[4-7]  = our bits 11-8
    //   IR[8-9]  = our bits 7-6
    let is_emulator_for_idisp: bool = i.current_task == bits::<4>(0);
    let is_idisp: bool = mi.f2 == F2Function::IDispatch;
    if is_emulator_for_idisp && is_idisp {
        let ir_0:   bool      = (q.ir & bits::<16>(0x8000)) != bits::<16>(0);
        let ir_8_9: Bits<10>  = ((q.ir >> 6)  & bits::<16>(0b11)).resize();
        let ir_1_2: Bits<10>  = ((q.ir >> 13) & bits::<16>(0b11)).resize();
        let ir_3_4: Bits<10>  = ((q.ir >> 11) & bits::<16>(0b11)).resize();
        let ir_4_7: Bits<10>  = ((q.ir >> 8)  & bits::<16>(0b1111)).resize();
        let dispatch_value: Bits<10> =
                 if ir_0                        { bits::<10>(3) - ir_8_9 }
            else if ir_1_2 == bits::<10>(0)     { ir_3_4 }
            else if ir_1_2 == bits::<10>(1)     { bits::<10>(4) }
            else if ir_1_2 == bits::<10>(2)     { bits::<10>(5) }
            else if ir_4_7 == bits::<10>(0)     { bits::<10>(1) }
            else if ir_4_7 == bits::<10>(1)     { bits::<10>(0) }
            else if ir_4_7 == bits::<10>(6)     { bits::<10>(0o16) }
            else if ir_4_7 == bits::<10>(0o16)  { bits::<10>(6) }
            else                                { ir_4_7 };
        next_modifier_this_cycle = next_modifier_this_cycle | dispatch_value;
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
            next_modifier_this_cycle = next_modifier_this_cycle | bits::<10>(1);
        }
        // Other F2 disk codes: stub 0 → no modification → no-op.
        // F2=DiskWordTransfer (= INIT in spec §8.5) gets the existing
        // DMA atomic dispatch in Disk Word task, plus NEXT-modify
        // semantics fall through to the WDTASKACT&WDINIT case which
        // is false in steady state.
    }

    // F2=ACSOURCE late dispatch (Emulator only, F2=14 binary): per spec
    // §6.6 + ContrAlto's EmulatorTask.cs ACSOURCE late handler.  ACSOURCE
    // has TWO roles: (1) early — override RSEL low 2 bits with (IR[1-2]
    // XOR 3), already done at the effective_rsel computation; and
    // (2) late — dispatch into NEXT based on IR.  This block handles (2).
    //
    // Table per spec / ContrAlto:
    //   if IR[0] = 1           → 3 - IR[8-9]      (Nova SHIFT complement)
    //   else (IR[0] = 0):
    //     if IR[1-2] != 3      → IR[5]            (the Indirect bit, OR'd in)
    //     dispatch on IR[3-7] (5 bits, 0..31):
    //       0  → 2  (CYCLE)
    //       1  → 5  (RAMTRAP)
    //       2  → 3  (NOPAR)
    //       3  → 6  (RAMTRAP)
    //       4  → 7  (RAMTRAP)
    //       9  → 4  (JSRII; 11B octal)
    //       10 → 4  (JSRIS; 12B octal)
    //       14 → 1  (CONVERT; 16B octal)
    //       31 → 15 (ROMTRAP/SWAT; 17B octal output)
    //       else → 14 (ROMTRAP; 16B octal output)
    //
    // Alto MSB=0 → our LSB=0 mapping:
    //   IR[0]   = bit 15
    //   IR[1-2] = bits 14-13
    //   IR[3-7] = bits 12-8 (5 bits)
    //   IR[5]   = bit 10
    //   IR[8-9] = bits 7-6
    let is_acsource: bool = mi.f2 == F2Function::Code14;
    if is_emulator_for_idisp && is_acsource {
        let acs_dispatch: Bits<10> = if (q.ir & bits::<16>(0x8000)) != bits::<16>(0) {
            // IR[0]=1: 3 - IR[8-9].
            let sh: Bits<10> = ((q.ir >> 6) & bits::<16>(0b11)).resize();
            bits::<10>(3) - sh
        } else {
            let ir_1_2_acs: Bits<10> = ((q.ir >> 13) & bits::<16>(0b11)).resize();
            let ir_5: Bits<10>       = ((q.ir >> 10) & bits::<16>(0b1)).resize();
            let ir_3_7: Bits<10>     = ((q.ir >> 8)  & bits::<16>(0b11111)).resize();
            let ind_bit: Bits<10> = if ir_1_2_acs != bits::<10>(3) { ir_5 } else { bits::<10>(0) };
            let dispatch: Bits<10> =
                     if ir_3_7 == bits::<10>(0)     { bits::<10>(2) }
                else if ir_3_7 == bits::<10>(1)     { bits::<10>(5) }
                else if ir_3_7 == bits::<10>(2)     { bits::<10>(3) }
                else if ir_3_7 == bits::<10>(3)     { bits::<10>(6) }
                else if ir_3_7 == bits::<10>(4)     { bits::<10>(7) }
                else if ir_3_7 == bits::<10>(9)     { bits::<10>(4) }
                else if ir_3_7 == bits::<10>(10)    { bits::<10>(4) }
                else if ir_3_7 == bits::<10>(14)    { bits::<10>(1) }
                else if ir_3_7 == bits::<10>(31)    { bits::<10>(15) }
                else                                { bits::<10>(14) };
            ind_bit | dispatch
        };
        next_modifier_this_cycle = next_modifier_this_cycle | acs_dispatch;
    }

    // F2=LoadIr (Emulator only, F2=12 binary, real spec name IR←):
    // per spec §6.6 + ContrAlto's EmulatorTask.cs, IR← merges BUS bits
    // 0,5,6,7 (Alto MSB=0 numbering) into NEXT.  In our LSB=0
    // encoding: BUS bit 15 → NEXT bit 3, BUS bits 10,9,8 → NEXT bits
    // 2,1,0.  This is the FIRST-LEVEL Nova instruction dispatch — the
    // jump table the emulator's fetch loop uses to land on the right
    // opcode handler.  WITHOUT this merge, IR← effectively does
    // nothing for dispatch and the Nova fetch loop stalls on the
    // wrong handler.
    if is_emulator_for_idisp && mi.f2 == F2Function::LoadIr {
        let merge_hi: Bits<10> = ((bus & bits::<16>(0x8000)) >> 12).resize();
        let merge_lo: Bits<10> = ((bus & bits::<16>(0x0700)) >>  8).resize();
        next_modifier_this_cycle = next_modifier_this_cycle | merge_hi | merge_lo;
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
        next_modifier_this_cycle = next_modifier_this_cycle | bus_lsb;
    }

    // Apply LAST cycle's pending modifier to THIS cycle's NEXT field,
    // and latch THIS cycle's modifier for NEXT cycle.  See spec
    // digest §2.3 for the disambiguation evidence.
    let next_addr: Bits<10> = mi.next | q.next_modifier_pending;
    d.next_modifier_pending = next_modifier_this_cycle;
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
    // Memory-pipeline-stall driver signals — pure decode of current
    // uinst (no dependence on stall state).  AltoChip routes these
    // into the Memory subsystem, which combinationally returns
    // mem_stall.  See `Memory` rustdoc for the FSM.
    o.mar_load_this_cycle = mi.f1 == F1Function::LoadMar;
    o.md_read_this_cycle  = mi.bs == BusSource::MemoryData;
    o.md_write_this_cycle = mi.f2 == F2Function::StoreMd;
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
    // F1=STARTF (Emulator-only, F1=17B/binary 15 per spec §6.6):
    // "The STARTF function is used by the SIO instruction, and is
    // used to define commands for I/O hardware, including the
    // Ethernet."  In Emulator (task 0), F1=15 is STARTF; in Disk
    // task it's WriteKdata (LoadKDATA per spec §8.5).  Per-task
    // gating distinguishes.  Surfaced as `startf` for the chip to
    // route to per-device boot/I/O triggers.
    o.startf = i.current_task == bits::<4>(0)
        && mi.f1 == F1Function::WriteKdata;
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
    // F2 = LoadIr + current_task == 0 (Emulator) → IR ← BUS.  Per
    // spec §6.6 + ContrAlto's EmulatorTask.cs (`_cpu._ir = _busData`),
    // IR loads from the BUS (the typical microcode is `IR← MD` which
    // sets BS=MemoryData driving BUS = MD).  In any other task, this
    // F2 code is a no-op.
    let is_emulator_task: bool = i.current_task == bits::<4>(0);
    let is_load_ir: bool = mi.f2 == F2Function::LoadIr;
    d.ir = if is_emulator_task && is_load_ir { bus } else { q.ir };
    o.ir = q.ir;

    // SKIP latch update per spec §6.6:
    //   - Cleared by F2=LoadIr.
    //   - Set by F2=LoadDNS based on IR low 3 bits (Nova SKP modes)
    //     and the post-shift result + dns_carry_out:
    //       0 SKP=none → SKIP=0
    //       1 SKP      → SKIP=1
    //       2 SZC      → SKIP=(carry_out == 0)
    //       3 SNC      → SKIP=(carry_out != 0)
    //       4 SZR      → SKIP=(result == 0)
    //       5 SNR      → SKIP=(result != 0)
    //       6 SEZ      → SKIP=(result == 0 || carry_out == 0)
    //       7 SBN      → SKIP=(result != 0 && carry_out != 0)
    let dns_skip_mode: Bits<3> = (q.ir & bits::<16>(0b111)).resize();
    let result_zero: bool = l_after_f1 == bits::<16>(0);
    let carry_zero: bool  = !dns_carry_out;
    let dns_new_skip: bool =
             if dns_skip_mode == bits::<3>(0) { false }
        else if dns_skip_mode == bits::<3>(1) { true }
        else if dns_skip_mode == bits::<3>(2) { carry_zero }
        else if dns_skip_mode == bits::<3>(3) { !carry_zero }
        else if dns_skip_mode == bits::<3>(4) { result_zero }
        else if dns_skip_mode == bits::<3>(5) { !result_zero }
        else if dns_skip_mode == bits::<3>(6) { result_zero || carry_zero }
        else                                  { !result_zero && !carry_zero };
    d.skip = if is_emulator_task && is_load_ir { false }
        else if is_dns                          { dns_new_skip }
        else                                    { q.skip };

    // CARRY latch update: F2=LoadDNS late phase writes back
    // dns_carry_out IFF R-write is enabled (per ContrAlto:
    // `if (_loadR) { _carry = carry; }`).  Without DNS, carry is held.
    d.carry = if is_dns && !dns_suppress_r { dns_carry_out } else { q.carry };

    // ---- Outputs ---------------------------------------------------
    o.t          = q.t;
    o.l          = q.l;
    o.bus        = bus;
    o.alu_result = aout.result;
    o.regs       = q.regs;

    // ---- Memory-pipeline stall gate -------------------------------
    //
    // Per AltoHW §2.3 + spec digest §3, the microengine must FREEZE
    // when the Memory subsystem signals `mem_stall`.  When stalled:
    //   - All internal DFFs hold their q values (T, L, R, MAR, IR,
    //     alu_carry, skip, carry, next_modifier_pending).
    //   - `o.next_mpc = i.mpc` (no MPC advance — the chip's owner
    //     won't update task_mpc).
    //   - Side-effect outputs (task_yield, mem_write_en, disk_*,
    //     startf, ...) suppressed so the stalled cycle has no
    //     visible effect on downstream subsystems.
    //   - Memory-pipeline driver signals (mar_load_this_cycle,
    //     md_read_this_cycle, md_write_this_cycle) suppressed too —
    //     they would re-trigger the FSM otherwise.
    //
    // The gate is OR'd with the reset gate below so reset takes
    // precedence (a reset during a stall is still a reset).
    if i.mem_stall && !cr.reset.any() {
        d.t = q.t;
        d.l = q.l;
        d.regs = q.regs;
        d.mar = q.mar;
        d.ir = q.ir;
        d.alu_carry = q.alu_carry;
        d.skip = q.skip;
        d.carry = q.carry;
        d.next_modifier_pending = q.next_modifier_pending;
        // Note: we do NOT override `o.next_mpc` to `i.mpc` here.  The
        // chip is responsible for holding its URom prefetch address
        // during a stall (via `urom_addr_held` DFF), so that the same
        // instruction re-presents to the engine.  If we set
        // `o.next_mpc = i.mpc`, the chip would prefetch URom @ i.mpc,
        // and any inconsistency between `i.mpc` and the actual
        // executing instruction's address (due to the
        // `task_started`/`task_mpc[]` arbitration interaction) would
        // cause the engine to jump to MPC=0.  Letting `next_mpc` pass
        // through as the engine's normal decoded value (the same
        // value across stall cycles since the same instruction is
        // re-executed) is correct — the chip's URom-addr holding
        // prevents the prefetch from advancing.
        o.task_yield = false;
        o.block_task = false;
        // NOTE: o.mar_load_this_cycle / o.md_read_this_cycle /
        // o.md_write_this_cycle are NOT suppressed here.  They are
        // pure decodes of i.instr (no dependence on i.mem_stall) and
        // they feed back into Memory's stall FSM.  Suppressing them
        // would create a combinational loop:
        //   stall=true → suppress mar_load → memory sees no MAR<- →
        //   memory clears stall → engine no longer stalls → mar_load
        //   re-asserts → stall=true → ... (settle loop fails to
        //   converge).
        // Keeping them high during stall is correct: the Memory FSM
        // gates `mar_fires_now = mar_load && !stall` so the counter
        // doesn't reset spuriously, and the signals are just "what
        // the current uinst wants to do" — stable across the stall.
        o.mem_write_en = false;
        o.disk_ctrl_write_en = false;
        o.disk_word_consumed = false;
        o.disk_strobe = false;
        o.disk_clr_stat = false;
        o.startf = false;
    }

    if cr.reset.any() {
        d.t = bits::<16>(0);
        d.l = bits::<16>(0);
        d.regs = [bits::<16>(0); 32];
        d.mar = bits::<16>(0);
        d.ir = bits::<16>(0);
        d.alu_carry = false;
        d.skip = false;
        d.carry = false;
        d.next_modifier_pending = bits::<10>(0);
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
        o.mar_load_this_cycle = false;
        o.md_read_this_cycle = false;
        o.md_write_this_cycle = false;
        o.block_task = false;
        o.disk_clr_stat = false;
        o.disk_strobe = false;
        o.startf = false;
        o.ir = bits::<16>(0);
        o.regs = [bits::<16>(0); 32];
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

