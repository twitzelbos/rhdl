//! Alto microinstruction format and per-field enums.
//!
//! Per the *Alto Hardware Manual* (Xerox PARC, 1976), each Alto
//! microinstruction is **32 bits** wide and encodes a complete
//! horizontal microcontrol step in a single word.
//!
//! ## Bit layout (Alto Hardware Manual, §2)
//!
//! ```text
//!  bit:  31 30 29 28 27 26 25 24 23 22 21 20 19 18 17 16 15 14 13 12 11 10  9  8  7  6  5  4  3  2  1  0
//!       +----------+----------+--------+----+----+----------+----------+----+----+----------------------+
//!       |   RSEL   |  ALUF    |  BS    | F1 | F2 |  T_LOAD  |  L_LOAD  |       NEXT (10 bits)            |
//!       |   5 bits |   4 bits | 3 bits | 4b | 4b |    1     |    1     |                                 |
//!       +----------+----------+--------+----+----+----------+----------+----------------------------------+
//! ```
//!
//! The actual hardware bit-positions vary slightly between revisions
//! of the Alto.  We pin the [Alto OS Release 19] layout (the version
//! ContrAlto uses) and document any divergence in the source.
//!
//! For the RHDL encoding we use a `Digital`-derived struct rather
//! than a raw `Bits<32>` — the type system can then enforce
//! valid-by-construction microinstructions, and the compiler emits
//! readable Verilog.  The packed-32-bit form is recoverable via
//! [`Microinstruction::pack`] / [`Microinstruction::unpack`] when
//! interfacing with a hand-assembled microcode binary.
//!
//! [Alto OS Release 19]: see Bitsavers archive of `alto/microcode/`.

use rhdl::prelude::*;

/// One of the 16 Alto ALU functions (4-bit ALUF field).  See
/// [`crate::alu::alu`] for the actual computation.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub enum AluFunction {
    /// `BUS` — pass the bus operand through.
    #[default]
    Bus,
    /// `T` — pass T through (for store-to-T patterns).
    T,
    /// `BUS OR T`.
    BusOrT,
    /// `BUS AND T`.
    BusAndT,
    /// `BUS XOR T`.
    BusXorT,
    /// `BUS + 1`.
    BusPlusOne,
    /// `BUS - 1`.
    BusMinusOne,
    /// `BUS + T`.
    BusPlusT,
    /// `BUS - T`.
    BusMinusT,
    /// `BUS - T - 1`.
    BusMinusTMinusOne,
    /// `BUS + T + 1`.
    BusPlusTPlusOne,
    /// `BUS + SKIP` — `BUS + 1` if SKIP signal asserted; else
    /// `BUS`.  SKIP is a runtime input (carry-out from previous
    /// ALU op or special-case skip from an F2 function).
    BusPlusSkip,
    /// `BUS, T` — alternate AND (Alto Hardware Manual §2.5.2).
    /// Bitwise AND, but with the side-effect of also writing
    /// `BUS AND T` to T (the F1 modifier sets this).
    BusAndTAlt,
    /// `BUS AND NOT T` — bit-mask: clear the bits of BUS that are
    /// set in T.
    BusAndNotT,
    /// Reserved (Alto Hardware Manual marks 14, 15 as undefined).
    /// Treated as `Bus` in our implementation.
    Undef14,
    Undef15,
}

/// Bus source — what drives the 16-bit BUS during the cycle (3-bit
/// BS field).  Some sources are *task-specific* (e.g., R7 for some
/// tasks routes a different signal); for Phase 1 only the
/// universal sources are exercised.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub enum BusSource {
    /// `R` — read R-register selected by the RSEL field.
    #[default]
    ReadR,
    /// `LOAD R` — used as a sink-only sentinel; the bus is not driven
    /// (reads the constant 0).  Paired with a write-side R load
    /// effected by F1 / F2.
    LoadR,
    /// `NONE` — bus is undriven (reads as 0, but reserved for
    /// task-dependent overrides).
    None,
    /// `TASK_SPECIFIC_3` — meaning depends on the running task.
    /// For Phase 1 we don't drive any task-specific source.
    TaskSpec3,
    /// `TASK_SPECIFIC_4` — same.
    TaskSpec4,
    /// `MD` — memory data.  In Phase 1 we treat this as 0 (the
    /// memory subsystem is added in Phase 3 with the Disk tasks).
    MemoryData,
    /// `MOUSE` — mouse-state register (task 10's input).
    Mouse,
    /// `IR` — instruction-register's low byte (Emulator-task usage).
    InstructionRegister,
}

/// F1 function — auxiliary control (4-bit F1 field).
///
/// Binary encoding matches the real Alto per ContrAlto's
/// `MicroInstruction.cs` `SpecialFunction1` enum and `EmulatorF1` /
/// `DiskF1` per-task enums:
///
/// | Bin | Universal (all tasks) | Emulator (task 0) | Disk Sector/Word (4, 14) |
/// |----:|-----------------------|-------------------|--------------------------|
/// | 0   | NOP                   |                   |                           |
/// | 1   | LoadMar (MAR← BUS)    |                   |                           |
/// | 2   | TaskYield (TASK)      |                   |                           |
/// | 3   | Block                 |                   |                           |
/// | 4   | LeftShift1 (LLSH1)    |                   |                           |
/// | 5   | RightShift1 (LRSH1)   |                   |                           |
/// | 6   | LeftCycle8 (LLCY8)    |                   |                           |
/// | 7   | Constant              |                   |                           |
/// | 8   | (per-task)            | SWMODE            | (undefined)               |
/// | 9   | (per-task)            | WRTRAM            | STROBE                    |
/// | 10  | (per-task)            | RDRAM             | LoadKSTAT                 |
/// | 11  | (per-task)            | LoadRMR           | INCRECNO                  |
/// | 12  | (per-task)            | (unused)          | CLRSTAT                   |
/// | 13  | (per-task)            | LoadESRB          | LoadKCOMM                 |
/// | 14  | (per-task)            | RSNF              | LoadKADR                  |
/// | 15  | (per-task)            | STARTF            | LoadKDATA                 |
///
/// Per-task variants are named after the **Disk-task** semantics
/// (LoadKCOMM/LoadKADR/LoadKDATA) since those are the ones our chip
/// actively implements; in Emulator context the same enum variant
/// represents a different operation (LoadESRB/RSNF/STARTF) that the
/// engine dispatches via the current_task gating.  Phase 3.5
/// implements Disk task semantics; Emulator per-task codes remain
/// gated no-ops until the boot path requires them.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub enum F1Function {
    /// `NOP` — F1 = 0.
    #[default]
    Nop,
    /// `MAR← BUS` — F1 = 1, universal.  Load Memory Address Register
    /// from BUS this cycle; memory read/write at the new MAR happens
    /// NEXT cycle (1-cycle BRAM latency).
    LoadMar,
    /// `TASK` — F1 = 2, universal.  Task yield: arbiter latches the
    /// highest-priority woken task into current_task at next edge.
    TaskYield,
    /// `BLOCK` — F1 = 3, universal.  Block this task (clear its
    /// wakeup signal).
    Block,
    /// `LLSH1` — F1 = 4, universal.  L ← L << 1.
    LeftShift1,
    /// `LRSH1` — F1 = 5, universal.  L ← L >> 1.
    RightShift1,
    /// `LLCY8` — F1 = 6, universal.  L ← rotate-left-by-8 of L.
    LeftCycle8,
    /// `CONSTANT` — F1 = 7, universal.  BUS driven from constant ROM
    /// indexed by `(rsel << 3) | bs` (8-bit index).
    Constant,
    /// F1 = 8 — per-task.  Emulator: SWMODE.  Disk: undefined.
    /// Currently no-op pending Emulator implementation.
    EmuSwMode,
    /// F1 = 9 — per-task.  Emulator: WRTRAM.  Disk: STROBE (start
    /// disk-word strobe sequence).  Currently no-op.
    Code9,
    /// F1 = 10 — per-task.  Emulator: RDRAM.  Disk: LoadKSTAT.
    /// Currently no-op.
    Code10,
    /// F1 = 11 — per-task.  Emulator: LoadRMR.  Disk: INCRECNO.
    /// Currently no-op.
    Code11,
    /// F1 = 12 — per-task.  Emulator: (unused).  Disk: CLRSTAT.
    /// Phase-3.5 simplification: also serves as our "WriteKcwa" for
    /// the simulated DMA path (KCWA← BUS, sets the DMA memory
    /// destination address).
    WriteKcwa,
    /// F1 = 13 — per-task.  Emulator: LoadESRB.  Disk: LoadKCOMM
    /// (KCOM← BUS, write disk command register).  Disk Sector task
    /// (4) only — gated no-op in any other task.
    WriteKcomm,
    /// F1 = 14 — per-task.  Emulator: RSNF.  Disk: LoadKADR (KADR←
    /// BUS, write disk address register: cylinder/head/sector).
    /// Disk Sector task (4) only.
    WriteKadr,
    /// F1 = 15 — per-task.  Emulator: STARTF (start I/O — the F1
    /// code the boot microcode uses to trigger the boot disk read).
    /// Disk: LoadKDATA (KDATA← BUS).  Currently dispatch is gated by
    /// task; Emulator's STARTF is no-op pending per-device wiring.
    WriteKdata,
}

/// F2 function — second auxiliary control (4-bit F2 field).
///
/// Binary encoding matches the real Alto per ContrAlto's
/// `MicroInstruction.cs` `SpecialFunction2` and per-task enums:
///
/// | Bin | Universal             | Emulator (task 0) | Disk (4, 14)              |
/// |----:|-----------------------|-------------------|--------------------------|
/// | 0   | None                  |                   |                           |
/// | 1   | BusEq0                |                   |                           |
/// | 2   | ShLt0                 |                   |                           |
/// | 3   | ShEq0                 |                   |                           |
/// | 4   | BusToNext (Bus)       |                   |                           |
/// | 5   | AluCarryToNext (ALUCY)|                   |                           |
/// | 6   | StoreMD (MD← BUS)     |                   |                           |
/// | 7   | Constant (mirror F1=7)|                   |                           |
/// | 8   | (per-task)            | BUSODD            | DiskInit (INIT)           |
/// | 9   | (per-task)            | MAGIC             | RWC                       |
/// | 10  | (per-task)            | LoadDNS           | RECNO                     |
/// | 11  | (per-task)            | ACDEST            | XFRDAT                    |
/// | 12  | (per-task)            | LoadIR            | SWRNRDY                   |
/// | 13  | (per-task)            | IDISP             | NFER                      |
/// | 14  | (per-task)            | ACSOURCE          | STROBON                   |
/// | 15  | (per-task)            | (unused)          | (unused)                  |
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub enum F2Function {
    /// `NOP` — F2 = 0.
    #[default]
    Nop,
    /// `BUSEQ0` — F2 = 1.  Modify NEXT's bit 0 if BUS == 0.
    BusEqZero,
    /// `SHLT0` — F2 = 2.  Modify NEXT's bit 0 if shifted-L's MSB == 0.
    ShiftLessThanZero,
    /// `SHEQ0` — F2 = 3.  Modify NEXT's bit 0 if shifted-L == 0.
    ShiftEqZero,
    /// `BUS` — F2 = 4.  OR low bits of BUS into NEXT (computed-go-to).
    BusToNext,
    /// `ALUCY` — F2 = 5.  Modify NEXT's bit 0 with ALU carry-out.
    AluCarryToNext,
    /// `MD← BUS` — F2 = 6, universal.  Write BUS to memory[MAR]
    /// this cycle.
    StoreMd,
    /// `CONSTANT` — F2 = 7, universal.  BUS driven from constant ROM,
    /// same path as F1 = Constant.  Either F1 = 7 or F2 = 7 triggers
    /// the constant-ROM lookup in real Alto.
    Constant,
    /// F2 = 8 — per-task.  Emulator: BUSODD.  Disk: DiskInit (INIT).
    /// Phase-3.5 reuse: in Disk Word task (14), this is the atomic
    /// per-word DMA trigger (memory[KCWA] ← disk_word_data; KCWA++).
    /// Real Alto uses the multi-cycle STROBE/KFER protocol; collapsed
    /// to one cycle here.
    DiskWordTransfer,
    /// F2 = 9 — per-task.  Emulator: MAGIC.  Disk: RWC.  No-op.
    Code9,
    /// F2 = 10 — per-task.  Emulator: LoadDNS.  Disk: RECNO.  No-op.
    Code10,
    /// F2 = 11 — per-task.  Emulator: ACDEST.  Disk: XFRDAT.  No-op.
    Code11,
    /// F2 = 12 — per-task.  Emulator: LoadIR (IR ← MD).  Disk: SWRNRDY.
    /// Phase-3.5 implements Emulator semantics; Disk semantics no-op.
    LoadIr,
    /// F2 = 13 — per-task.  Emulator: IDISP (Instruction Dispatch —
    /// OR IR[7:0] into NEXT[7:0]).  Disk: NFER.  Phase-3.5 implements
    /// Emulator semantics.
    IDispatch,
    /// F2 = 14 — per-task.  Emulator: ACSOURCE.  Disk: STROBON.  No-op.
    Code14,
    /// F2 = 15 — per-task.  Both Emulator and Disk use this slot
    /// differently or leave unused.  No-op.
    Code15,
}

/// Decoded microinstruction.  The pure-Rust representation we use
/// inside RHDL kernels and tests.  A 32-bit packed form
/// ([`Microinstruction::pack`]) is what the microcode RAM stores.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub struct Microinstruction {
    /// R-register select (5 bits) — index into the 32-entry
    /// R-register file.  Used by ReadR (BS = 0) and by some F1
    /// codes that load R.
    pub rsel: Bits<5>,
    /// ALU function (4 bits, 16 options).
    pub aluf: AluFunction,
    /// Bus source (3 bits, 8 options).
    pub bs: BusSource,
    /// F1 auxiliary function (4 bits, 16 options).
    pub f1: F1Function,
    /// F2 auxiliary function (4 bits, 16 options).
    pub f2: F2Function,
    /// `T ← BUS` if true.  T-load enable.
    pub t_load: bool,
    /// `L ← ALU result` if true.  L-load enable.  When false, the
    /// ALU result is computed but not latched into L.
    pub l_load: bool,
    /// Next microinstruction address (10 bits → 1024-entry RAM).
    /// May be modified by F2 functions before commit.
    pub next: Bits<10>,
}

impl Microinstruction {
    /// Pack into the 32-bit form stored in microcode RAM.
    /// Layout (MSB-to-LSB):
    ///   `[31:27] rsel`, `[26:23] aluf`, `[22:20] bs`,
    ///   `[19:16] f1`, `[15:12] f2`, `[11] t_load`, `[10] l_load`,
    ///   `[9:0]  next`.
    pub fn pack(&self) -> u32 {
        let rsel = (self.rsel.raw() as u32 & 0x1F) << 27;
        let aluf = (alu_function_index(self.aluf) as u32 & 0xF) << 23;
        let bs = (bus_source_index(self.bs) as u32 & 0x7) << 20;
        let f1 = (f1_function_index(self.f1) as u32 & 0xF) << 16;
        let f2 = (f2_function_index(self.f2) as u32 & 0xF) << 12;
        let t_load = (self.t_load as u32) << 11;
        let l_load = (self.l_load as u32) << 10;
        let next = self.next.raw() as u32 & 0x3FF;
        rsel | aluf | bs | f1 | f2 | t_load | l_load | next
    }

    /// Inverse of [`Microinstruction::pack`].  Used to load microcode
    /// binaries from the original Alto sources.
    pub fn unpack(word: u32) -> Self {
        Self {
            rsel: bits::<5>(((word >> 27) & 0x1F) as u128),
            aluf: alu_function_from_index(((word >> 23) & 0xF) as u8),
            bs: bus_source_from_index(((word >> 20) & 0x7) as u8),
            f1: f1_function_from_index(((word >> 16) & 0xF) as u8),
            f2: f2_function_from_index(((word >> 12) & 0xF) as u8),
            t_load: ((word >> 11) & 0x1) != 0,
            l_load: ((word >> 10) & 0x1) != 0,
            next: bits::<10>((word & 0x3FF) as u128),
        }
    }
}

// ---- enum <-> index conversions (compile-time stable) ------------

fn alu_function_index(f: AluFunction) -> u8 {
    match f {
        AluFunction::Bus => 0,
        AluFunction::T => 1,
        AluFunction::BusOrT => 2,
        AluFunction::BusAndT => 3,
        AluFunction::BusXorT => 4,
        AluFunction::BusPlusOne => 5,
        AluFunction::BusMinusOne => 6,
        AluFunction::BusPlusT => 7,
        AluFunction::BusMinusT => 8,
        AluFunction::BusMinusTMinusOne => 9,
        AluFunction::BusPlusTPlusOne => 10,
        AluFunction::BusPlusSkip => 11,
        AluFunction::BusAndTAlt => 12,
        AluFunction::BusAndNotT => 13,
        AluFunction::Undef14 => 14,
        AluFunction::Undef15 => 15,
    }
}
fn alu_function_from_index(i: u8) -> AluFunction {
    match i & 0xF {
        0 => AluFunction::Bus,
        1 => AluFunction::T,
        2 => AluFunction::BusOrT,
        3 => AluFunction::BusAndT,
        4 => AluFunction::BusXorT,
        5 => AluFunction::BusPlusOne,
        6 => AluFunction::BusMinusOne,
        7 => AluFunction::BusPlusT,
        8 => AluFunction::BusMinusT,
        9 => AluFunction::BusMinusTMinusOne,
        10 => AluFunction::BusPlusTPlusOne,
        11 => AluFunction::BusPlusSkip,
        12 => AluFunction::BusAndTAlt,
        13 => AluFunction::BusAndNotT,
        14 => AluFunction::Undef14,
        _ => AluFunction::Undef15,
    }
}

fn bus_source_index(b: BusSource) -> u8 {
    match b {
        BusSource::ReadR => 0,
        BusSource::LoadR => 1,
        BusSource::None => 2,
        BusSource::TaskSpec3 => 3,
        BusSource::TaskSpec4 => 4,
        BusSource::MemoryData => 5,
        BusSource::Mouse => 6,
        BusSource::InstructionRegister => 7,
    }
}
fn bus_source_from_index(i: u8) -> BusSource {
    match i & 0x7 {
        0 => BusSource::ReadR,
        1 => BusSource::LoadR,
        2 => BusSource::None,
        3 => BusSource::TaskSpec3,
        4 => BusSource::TaskSpec4,
        5 => BusSource::MemoryData,
        6 => BusSource::Mouse,
        _ => BusSource::InstructionRegister,
    }
}

fn f1_function_index(f: F1Function) -> u8 {
    match f {
        F1Function::Nop => 0,
        F1Function::LoadMar => 1,
        F1Function::TaskYield => 2,
        F1Function::Block => 3,
        F1Function::LeftShift1 => 4,
        F1Function::RightShift1 => 5,
        F1Function::LeftCycle8 => 6,
        F1Function::Constant => 7,
        F1Function::EmuSwMode => 8,
        F1Function::Code9 => 9,
        F1Function::Code10 => 10,
        F1Function::Code11 => 11,
        F1Function::WriteKcwa => 12,
        F1Function::WriteKcomm => 13,
        F1Function::WriteKadr => 14,
        F1Function::WriteKdata => 15,
    }
}
fn f1_function_from_index(i: u8) -> F1Function {
    match i & 0xF {
        0 => F1Function::Nop,
        1 => F1Function::LoadMar,
        2 => F1Function::TaskYield,
        3 => F1Function::Block,
        4 => F1Function::LeftShift1,
        5 => F1Function::RightShift1,
        6 => F1Function::LeftCycle8,
        7 => F1Function::Constant,
        8 => F1Function::EmuSwMode,
        9 => F1Function::Code9,
        10 => F1Function::Code10,
        11 => F1Function::Code11,
        12 => F1Function::WriteKcwa,
        13 => F1Function::WriteKcomm,
        14 => F1Function::WriteKadr,
        _ => F1Function::WriteKdata,
    }
}

fn f2_function_index(f: F2Function) -> u8 {
    match f {
        F2Function::Nop => 0,
        F2Function::BusEqZero => 1,
        F2Function::ShiftLessThanZero => 2,
        F2Function::ShiftEqZero => 3,
        F2Function::BusToNext => 4,
        F2Function::AluCarryToNext => 5,
        F2Function::StoreMd => 6,
        F2Function::Constant => 7,
        F2Function::DiskWordTransfer => 8,
        F2Function::Code9 => 9,
        F2Function::Code10 => 10,
        F2Function::Code11 => 11,
        F2Function::LoadIr => 12,
        F2Function::IDispatch => 13,
        F2Function::Code14 => 14,
        F2Function::Code15 => 15,
    }
}
fn f2_function_from_index(i: u8) -> F2Function {
    match i & 0xF {
        0 => F2Function::Nop,
        1 => F2Function::BusEqZero,
        2 => F2Function::ShiftLessThanZero,
        3 => F2Function::ShiftEqZero,
        4 => F2Function::BusToNext,
        5 => F2Function::AluCarryToNext,
        6 => F2Function::StoreMd,
        7 => F2Function::Constant,
        8 => F2Function::DiskWordTransfer,
        9 => F2Function::Code9,
        10 => F2Function::Code10,
        11 => F2Function::Code11,
        12 => F2Function::LoadIr,
        13 => F2Function::IDispatch,
        14 => F2Function::Code14,
        _ => F2Function::Code15,
    }
}
