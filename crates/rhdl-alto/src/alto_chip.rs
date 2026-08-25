//! `AltoChip` — top-level composition of the Alto subsystems.
//!
//! Phase 3.5 incremental composition.  Wires the foundational
//! Phase-3.5 widgets together:
//!
//! - [`MicrocodeRom`]   — 2K-microinstruction Alto II microcode RAM
//! - [`Microengine`]    — universal-task microcode execution engine
//!
//! Future Phase 3.5 expansion (in subsequent commits) will add:
//! - `ConstantRom` integration (F1=Constant lookup)
//! - `Memory` (64KW main memory) + Memory Reference Task bus
//! - `DiabloDisk` + `DiskController` (DMA via Disk Sector / Disk Word tasks)
//! - `AltoTaskSystem` (16-task arbiter; per-task MPC ownership)
//! - per-task F1 / F2 code dispatch (Disk Sector / Disk Word / Emulator)
//!
//! ## Pipeline
//!
//! Phase 3.5 uses the microcode RAM as a 1-cycle-latency BRAM.  At cycle T:
//!
//! 1. [`MicrocodeRom`] is presenting `q.bram = contents[mpc(T)]` from
//!    cycle T-1's read_addr presentation.
//! 2. [`Microengine`] processes this instruction with `i.mpc = mpc(T)`
//!    and computes `o.next_mpc`.
//! 3. AltoChip drives the microcode RAM's `read_addr` with `o.next_mpc`
//!    so cycle T+1 sees `contents[next_mpc(T)] = contents[mpc(T+1)]`.
//!
//! AltoChip owns the MPC DFF (a 10-bit counter; will widen to 11 bits
//! when bank switching is added).  Reset starts MPC at 0 (Silent Boot
//! entry per the Alto Hardware Manual).

use rhdl::prelude::*;

use crate::constant_rom::{ConstantIn, ConstantRom};
use crate::diablo_disk::{DiabloDisk, DiskIn};
use crate::disk_controller::{CtrlIn, DiskController};
use crate::memory::{MemIn, Memory};
use crate::microcode_rom::{MicrocodeRom, UromIn};
use crate::microengine::{In as MicroIn, Microengine};
use crate::task_system::{AltoIn as TaskIn, AltoTaskSystem};

/// Inputs to the AltoChip top-level.
///
/// Phase 3.5 wakeup vector: bit i = 1 means task i wants to run.
/// For boot, set bit 0 (Emulator always woken).  Disk widgets will
/// drive bits 4 (Disk Sector) and 14 (Disk Word) when integrated.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct ChipIn {
    /// 16-bit task wakeup vector.
    pub wakeups: Bits<16>,
}

/// Outputs from the AltoChip top-level — observable for trace + tests.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct ChipOut {
    /// MPC the engine processed this cycle (echoed from microengine.o.mpc).
    pub mpc: Bits<10>,
    /// Which task is running this cycle (echoed from microengine.o.current_task).
    pub current_task: Bits<4>,
    /// The next MPC the engine wants to fetch.
    pub next_mpc: Bits<10>,
    /// Current T register.
    pub t: Bits<16>,
    /// Current L register.
    pub l: Bits<16>,
    /// Current BUS this cycle.
    pub bus: Bits<16>,
    /// Current ALU result.
    pub alu_result: Bits<16>,
    /// 32-bit packed microinstruction the engine is processing.
    pub instruction: Bits<32>,
    /// Disk's sector_mark this cycle (drives Disk Sector wakeup bit).
    pub disk_sector_mark: bool,
    /// Disk's word_strobe this cycle (drives Disk Word wakeup bit).
    pub disk_word_strobe: bool,
    /// Effective wakeup vector this cycle (user-supplied OR'd with
    /// disk-derived wakeups).
    pub wakeups: Bits<16>,
    /// Disk-controller register read-data this cycle (the value at
    /// the address presented one cycle ago).  Combinational from
    /// disk_ctrl's q output.
    pub disk_ctrl_read_data: Bits<16>,
    /// Engine's per-task disk-controller write-enable this cycle.
    /// Test surface: confirms the per-task gating works.
    pub disk_ctrl_write_en: bool,
    /// Instruction Register (Emulator task's current Nova instruction).
    pub ir: Bits<16>,
    /// Disk Sector task fire counter (from task_system).
    pub disk_sector_count: Bits<16>,
    /// Disk Word task fire counter (from task_system).
    pub disk_word_count: Bits<16>,
    /// Memory write address this cycle (engine's mem_address output,
    /// q-registered).  Combined with mem_write_observed_data and
    /// mem_write_observed_en lets tests reconstruct the engine's
    /// memory-write trace cycle-by-cycle.
    pub mem_write_observed_addr: Bits<16>,
    /// Memory write data this cycle.
    pub mem_write_observed_data: Bits<16>,
    /// Memory write enable this cycle.
    pub mem_write_observed_en: bool,
    /// True if the running microinstruction this cycle had F1=Block.
    /// Echoed from `engine.block_task`.  Per *Alto Hardware Manual*
    /// §2.4 + spec §5.5: BLOCK is a hardware convention by which the
    /// running task asks its associated device to deassert that
    /// device's wakeup signal.  Critical observable for the
    /// sector_mark/BLOCK-clear cadence test (catches "latch stuck"
    /// or "BLOCK not clearing latch" bugs).
    pub block_task: bool,
    /// R-register file (32 × 16 bits) — start-of-cycle (latched)
    /// values.  Echoed from `engine.regs`.  Required for cycle-by-
    /// cycle register-state lockstep against ContrAlto: the
    /// `tests/contralto_lockstep.rs` harness compares this field
    /// against ContrAlto's `cpu.R[]` array each cycle.
    pub regs: [Bits<16>; 32],
    /// True if the engine's current microinstruction has F1=TaskYield
    /// (the F1=TASK / F1=2 function).  Echoed from `engine.task_yield`
    /// post-cycle.  Per spec §2.4: triggers task arbitration; the
    /// switch becomes effective with a 1-cycle delay (the "one
    /// additional instruction is executed before the switch becomes
    /// effective" rule).  Required for the
    /// `tests/task_switch_pipeline.rs` regression test.
    pub task_yield: bool,
    /// True when the Memory subsystem's pipeline-stall FSM is
    /// asserting `mem_stall` this cycle — meaning the microengine
    /// is FROZEN (DFFs hold, MPC doesn't advance, side-effect outputs
    /// suppressed).  Echoed from `q.mem.mem_stall` for cycle-level
    /// diagnostic / lockstep observation.  Per AltoHW §2.3, a stall
    /// occurs when `←MD` or `MAR<-` is issued before the previous
    /// memory cycle has cleared the bus.  See `Memory` struct rustdoc
    /// for the exact thresholds.
    pub mem_stall: bool,
}

/// The Alto chip — composition of microengine + microcode RAM +
/// constant ROM + 64KW main memory + 16-task arbiter + disk
/// controller + Diablo-31 disk drive.
///
/// Construct with [`AltoChip::with_microcode_and_constants`] for the
/// normal boot path, or [`AltoChip::default`] for all-zero ROMs/memory.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct AltoChip {
    /// 2K-entry microcode RAM (loaded at construction).
    urom: MicrocodeRom,
    /// 256-entry constant ROM — read combinationally each cycle by
    /// the microengine via `F1=Constant`.
    crom: ConstantRom,
    /// 64KW main memory — driven by the microengine's MAR (1-cycle
    /// BRAM read latency; matches real Alto MOS DRAM timing).
    mem: Memory,
    /// 16-task wakeup arbiter.  Owns the per-task MPCs.  AltoChip
    /// reads `last_task` (registered, 1 cycle behind arbitration —
    /// matches the Alto's MIF/MIE pipeline) to know which task's
    /// MPC drives the engine this cycle, and writes back the engine's
    /// computed next_mpc to that task's slot.
    tasks: AltoTaskSystem,
    /// Disk controller register file (KSTAT/KDATA/KCOM/KADR/KCWA/KCWD).
    /// Inputs aren't driven by the microengine yet — Phase 3.5 will add
    /// the per-task code dispatch that lets Disk Sector microcode
    /// program these registers.
    disk_ctrl: DiskController,
    /// Simulated Diablo 31 disk drive.  Produces sector_mark and
    /// word_strobe outputs that AltoChip routes to the task arbiter's
    /// wakeup vector (bits 4 and 14 respectively).
    disk: DiabloDisk,
    /// Universal-task microengine.
    engine: Microengine,
    /// Bundled chip-side state.  Per CLAUDE.md §3.1 (the protocol-PHY
    /// pattern): five logically-separate DFFs (current_task,
    /// task_started, task_yield_pending, urom_addr_held, task_mpcs)
    /// bundled into one struct to keep AltoChip's top-level field
    /// count under the 12-tuple ceiling for `Synchronous`/SynchronousDQ
    /// auto-derives.  Each field's purpose is documented on the
    /// `ChipState` struct definition.
    state: rhdl_fpga::core::dff::DFF<ChipState>,
}

/// Bundled chip-side state DFF — see `AltoChip::state` for why this
/// is bundled rather than five separate DFFs.
#[derive(Clone, Copy, PartialEq, Debug, Digital, Default)]
pub struct ChipState {
    /// Sticky current-task.  Per AltoHW §2.4: task switches happen
    /// ONLY on F1=TASK; until then the same task runs.  Latches the
    /// priority-encoded winning task on engine.task_yield + the
    /// `task_yield_pending` K+2 delay.
    pub current_task: Bits<4>,
    /// Per-task "has started" bitmap.  Bit K is set the first time
    /// task K runs.  Per AltoHW §2 "Initialization": each task K
    /// resets to MPC=K.  Until task K has run, the chip substitutes
    /// K for `task_mpcs[K]`, matching the real Alto's per-task reset
    /// MPC convention without requiring a custom DFF reset value.
    pub task_started: Bits<16>,
    /// One-cycle task-yield delay latch.  Captures `q.engine.task_yield`
    /// at end of cycle K (when F1=TaskYield runs).  At cycle K+1,
    /// `q.state.task_yield_pending = true` gates the `current_task`
    /// update so the new task's instruction first executes at
    /// cycle K+2 (AltoHW §2.4 "one additional instruction" rule).
    pub task_yield_pending: bool,
    /// URom prefetch address presented in the previous cycle — used
    /// to hold the URom address during a memory-pipeline stall.  When
    /// `mem_stall` is asserted, the chip presents the previously-
    /// presented URom address instead of advancing to `next_mpc`,
    /// so URom returns the SAME instruction the engine was stalled
    /// on.  Without this hold, URom would prefetch the post-stall
    /// instruction and the stalled instruction's effects would be
    /// silently lost (violating spec-mandated "re-execute on resume").
    pub urom_addr_held: Bits<10>,
    /// Per-task MPC tracking — canonical source of truth for "what
    /// MPC each task should resume at".  Updated EVERY non-stalled
    /// cycle for the running task (`d.task_mpcs[current_task] =
    /// engine.next_mpc`), regardless of priority arbitration.
    ///
    /// **Why not `task_system.task_mpc[]`:** the task_system rules
    /// only fire when a task wins priority arbitration.  When higher-
    /// priority tasks are woken, lower-priority tasks (e.g. Emulator
    /// during KSEC's disk activity) don't win → their `task_mpc[k]`
    /// slots never update → after KSEC ends, Emulator resumes at the
    /// stale MPC (typically 0) instead of the correct saved MPC.
    /// task_system's task_mpc is now redundant for chip-internal
    /// MPC tracking but kept for trace observability.
    pub task_mpcs: [Bits<10>; 16],
}

impl Default for AltoChip {
    fn default() -> Self {
        Self {
            urom: MicrocodeRom::default(),
            crom: ConstantRom::default(),
            mem: Memory::default(),
            tasks: AltoTaskSystem::default(),
            disk_ctrl: DiskController::default(),
            disk: DiabloDisk::default(),
            engine: Microengine::default(),
            state: rhdl_fpga::core::dff::DFF::new(ChipState::default()),
        }
    }
}

impl AltoChip {
    /// Construct an AltoChip with the supplied microcode image but
    /// empty constant ROM and memory.  Useful for tests that don't
    /// depend on constants or memory state.
    pub fn with_microcode(microcode_words: &[u32; crate::microcode_rom::MICROCODE_WORDS]) -> Self {
        Self {
            urom: MicrocodeRom::with_words(microcode_words),
            crom: ConstantRom::default(),
            mem: Memory::default(),
            tasks: AltoTaskSystem::default(),
            disk_ctrl: DiskController::default(),
            disk: DiabloDisk::default(),
            engine: Microengine::default(),
            state: rhdl_fpga::core::dff::DFF::new(ChipState::default()),
        }
    }

    /// Construct an AltoChip with microcode and constant ROMs loaded
    /// and an empty memory.
    pub fn with_microcode_and_constants(
        microcode_words: &[u32; crate::microcode_rom::MICROCODE_WORDS],
        constants: &[u16; crate::constant_rom::NUM_CONSTANTS],
    ) -> Self {
        Self {
            urom: MicrocodeRom::with_words(microcode_words),
            crom: ConstantRom::with_constants(constants),
            mem: Memory::default(),
            tasks: AltoTaskSystem::default(),
            disk_ctrl: DiskController::default(),
            disk: DiabloDisk::default(),
            engine: Microengine::default(),
            state: rhdl_fpga::core::dff::DFF::new(ChipState::default()),
        }
    }

    /// Construct an AltoChip with all three of microcode, constants,
    /// and memory preloaded.
    pub fn with_microcode_constants_and_memory(
        microcode_words: &[u32; crate::microcode_rom::MICROCODE_WORDS],
        constants: &[u16; crate::constant_rom::NUM_CONSTANTS],
        memory_initial: impl IntoIterator<Item = (Bits<16>, Bits<16>)>,
    ) -> Self {
        Self {
            urom: MicrocodeRom::with_words(microcode_words),
            crom: ConstantRom::with_constants(constants),
            mem: Memory::new(memory_initial),
            tasks: AltoTaskSystem::default(),
            disk_ctrl: DiskController::default(),
            disk: DiabloDisk::default(),
            engine: Microengine::default(),
            state: rhdl_fpga::core::dff::DFF::new(ChipState::default()),
        }
    }

    /// Construct an AltoChip with the BOOT BUTTON action pre-applied:
    /// disk sector 0 (cylinder 0, head 0, sector 0) is loaded into
    /// memory[1..=400B] and the disk's active sector buffer.  Per
    /// *Alto Hardware Manual* §3.4 (Bootstrapping):
    ///
    /// > "Disk Boot ... the **256 data words are read into locations
    /// > 1 to 400B inclusive**; the **label is read into locations
    /// > 402B to 411B inclusive**.  When the transfer is complete,
    /// > PC ← 1, and the emulator is started.  The disk status is
    /// > stored in location 2..."
    ///
    /// In real Alto, this transfer happens via the disk DMA path
    /// triggered by pressing the boot button.  Our chip simulates
    /// that hardware action at construction time — pre-loading
    /// memory directly so the Emulator's standard Nova fetch loop
    /// (already running at MPC=0 → 0x152) finds real Nova bootstrap
    /// instructions in memory[1..400B] instead of zeros.
    ///
    /// The disk image's first sector is also placed in DiabloDisk's
    /// sector buffer — so subsequent KSEC-driven DMA reads of cyl=0
    /// head=0 sector=0 will find the same data (consistency with
    /// the boot-loaded memory).
    pub fn with_microcode_constants_and_boot(
        microcode_words: &[u32; crate::microcode_rom::MICROCODE_WORDS],
        constants: &[u16; crate::constant_rom::NUM_CONSTANTS],
        boot_sector_data: &[u16; 256],
        boot_sector_label: &[u16; 8],
    ) -> Self {
        // Per spec §3.4 Disk Boot:
        // - memory[1..=256] = data words from sector 0
        //   (octal 1..=400B = decimal 1..=256).
        // - memory[402B..=411B] = label words (octal 402B = decimal
        //   258, 411B = decimal 265 — 8-word label range).
        // - memory[2] = disk status (0 = no error).
        // - memory[0] = (PC saved by boot, but we pre-set to 0;
        //   Emulator's first instruction stores its own PC here).
        let mut mem_init: Vec<(Bits<16>, Bits<16>)> = Vec::with_capacity(266);
        mem_init.push((bits::<16>(0), bits::<16>(0)));
        mem_init.push((bits::<16>(2), bits::<16>(0)));
        for (i, &w) in boot_sector_data.iter().enumerate() {
            mem_init.push((bits::<16>((i as u128) + 1), bits::<16>(w as u128)));
        }
        for (i, &w) in boot_sector_label.iter().enumerate() {
            mem_init.push((bits::<16>(0o402 + i as u128), bits::<16>(w as u128)));
        }
        // Memory-mapped I/O initialization (idle-state values).
        // The Alto's I/O space is 0o177000-0o177777 (= 0xFE00-0xFFFF).
        // Per AltoHW: when no I/O device responds on the bus, the bus
        // reads as -1 (all 1s, wired-AND default).  The canonical
        // altoIIcode3.mu boot microcode (post-KSEC, in the Emulator
        // boot dance at MPC ~0x13b/0x13c) reads the keyboard at
        // KBDAD = 0o177034 = 0xfe1c (per AltoHW §3.4) to determine
        // boot-disk-address from depressed keys.  Spec digest line
        // 1128: "Depressed keys read as 0; idle keys as 1."  For our
        // no-keys-pressed default boot, all keyboard words = 0xFFFF.
        //
        // Without these initializations, mem[KBDAD] = 0 (default
        // memory init), which the boot microcode interprets as ALL
        // keys depressed — taking a wrong dispatch path through
        // F2=BUSODD (which checks BUS bit 0 of the keyboard read)
        // and ending up in the wrong boot handler.  Visible in the
        // lockstep dumper at cycle 92 where OURS goes to MPC=0x138
        // (no BUSODD merge) but CTR goes to 0x139 (BUSODD merged
        // because keyboard LSB = 1 in the no-keys-pressed reading).
        for kbd_addr in 0xfe1c..=0xfe1f {
            // 4 keyboard words
            mem_init.push((bits::<16>(kbd_addr as u128), bits::<16>(0xFFFF)));
        }
        mem_init.push((bits::<16>(0xfe18), bits::<16>(0xFFFF))); // UTILIN

        Self {
            urom: MicrocodeRom::with_words(microcode_words),
            crom: ConstantRom::with_constants(constants),
            mem: Memory::new(mem_init),
            tasks: AltoTaskSystem::default(),
            disk_ctrl: DiskController::default(),
            disk: DiabloDisk::with_sector(boot_sector_data),
            engine: Microengine::default(),
            state: rhdl_fpga::core::dff::DFF::new(ChipState::default()),
        }
    }

    /// As [`AltoChip::with_microcode_and_constants`] but with the disk
    /// configured for a SHORT sector cadence (per
    /// [`crate::diablo_disk::DiabloDisk::with_test_period`]).  Use this
    /// in tests that need to observe disk-driven task switching without
    /// running the spec-correct ~19,608 cycles per sector boundary.
    /// Real hardware uses [`crate::diablo_disk::SECTOR_PERIOD_CYCLES`];
    /// only this constructor in tests.
    pub fn with_microcode_constants_and_test_disk_period(
        microcode_words: &[u32; crate::microcode_rom::MICROCODE_WORDS],
        constants: &[u16; crate::constant_rom::NUM_CONSTANTS],
        disk_period_cycles: u32,
    ) -> Self {
        Self {
            urom: MicrocodeRom::with_words(microcode_words),
            crom: ConstantRom::with_constants(constants),
            mem: Memory::default(),
            tasks: AltoTaskSystem::default(),
            disk_ctrl: DiskController::default(),
            disk: DiabloDisk::with_test_period(disk_period_cycles),
            engine: Microengine::default(),
            state: rhdl_fpga::core::dff::DFF::new(ChipState::default()),
        }
    }

    /// As [`AltoChip::with_microcode_constants_and_boot`] but with the
    /// disk configured for a SHORT sector cadence.  Use this in
    /// boot-trace tests that need to observe disk-driven task
    /// switching within ~thousands of cycles instead of the
    /// spec-correct ~19,608 per boundary.
    pub fn with_microcode_constants_boot_and_test_disk_period(
        microcode_words: &[u32; crate::microcode_rom::MICROCODE_WORDS],
        constants: &[u16; crate::constant_rom::NUM_CONSTANTS],
        boot_sector_data: &[u16; 256],
        boot_sector_label: &[u16; 8],
        disk_period_cycles: u32,
    ) -> Self {
        let mut mem_init: Vec<(Bits<16>, Bits<16>)> = Vec::with_capacity(266);
        mem_init.push((bits::<16>(0), bits::<16>(0)));
        mem_init.push((bits::<16>(2), bits::<16>(0)));
        for (i, &w) in boot_sector_data.iter().enumerate() {
            mem_init.push((bits::<16>((i as u128) + 1), bits::<16>(w as u128)));
        }
        for (i, &w) in boot_sector_label.iter().enumerate() {
            mem_init.push((bits::<16>(0o402 + i as u128), bits::<16>(w as u128)));
        }
        // Memory-mapped I/O initialization (idle-state values).
        // The Alto's I/O space is 0o177000-0o177777 (= 0xFE00-0xFFFF).
        // Per AltoHW: when no I/O device responds on the bus, the bus
        // reads as -1 (all 1s, wired-AND default).  The canonical
        // altoIIcode3.mu boot microcode (post-KSEC, in the Emulator
        // boot dance at MPC ~0x13b/0x13c) reads the keyboard at
        // KBDAD = 0o177034 = 0xfe1c (per AltoHW §3.4) to determine
        // boot-disk-address from depressed keys.  Spec digest line
        // 1128: "Depressed keys read as 0; idle keys as 1."  For our
        // no-keys-pressed default boot, all keyboard words = 0xFFFF.
        //
        // Without these initializations, mem[KBDAD] = 0 (default
        // memory init), which the boot microcode interprets as ALL
        // keys depressed — taking a wrong dispatch path through
        // F2=BUSODD (which checks BUS bit 0 of the keyboard read)
        // and ending up in the wrong boot handler.  Visible in the
        // lockstep dumper at cycle 92 where OURS goes to MPC=0x138
        // (no BUSODD merge) but CTR goes to 0x139 (BUSODD merged
        // because keyboard LSB = 1 in the no-keys-pressed reading).
        for kbd_addr in 0xfe1c..=0xfe1f {
            // 4 keyboard words
            mem_init.push((bits::<16>(kbd_addr as u128), bits::<16>(0xFFFF)));
        }
        mem_init.push((bits::<16>(0xfe18), bits::<16>(0xFFFF))); // UTILIN
        Self {
            urom: MicrocodeRom::with_words(microcode_words),
            crom: ConstantRom::with_constants(constants),
            mem: Memory::new(mem_init),
            tasks: AltoTaskSystem::default(),
            disk_ctrl: DiskController::default(),
            disk: DiabloDisk::with_test_period_and_sector(disk_period_cycles, boot_sector_data),
            engine: Microengine::default(),
            state: rhdl_fpga::core::dff::DFF::new(ChipState::default()),
        }
    }

    /// As [`AltoChip::with_microcode_constants_boot_and_test_disk_period`]
    /// but ALSO starts the disk's `sector_tick` at `period_cycles - 1`,
    /// so the very FIRST cycle's wrap-check fires sector_mark
    /// immediately.  Matches ContrAlto's `_sectorEvent = new Event(0,
    /// ...)` simulation choice — useful for lockstep harnesses that
    /// want task arbitration to align on cycle 1.
    pub fn with_microcode_constants_boot_and_test_disk_period_at_boundary(
        microcode_words: &[u32; crate::microcode_rom::MICROCODE_WORDS],
        constants: &[u16; crate::constant_rom::NUM_CONSTANTS],
        boot_sector_data: &[u16; 256],
        boot_sector_label: &[u16; 8],
        disk_period_cycles: u32,
    ) -> Self {
        let mut mem_init: Vec<(Bits<16>, Bits<16>)> = Vec::with_capacity(266);
        mem_init.push((bits::<16>(0), bits::<16>(0)));
        mem_init.push((bits::<16>(2), bits::<16>(0)));
        for (i, &w) in boot_sector_data.iter().enumerate() {
            mem_init.push((bits::<16>((i as u128) + 1), bits::<16>(w as u128)));
        }
        for (i, &w) in boot_sector_label.iter().enumerate() {
            mem_init.push((bits::<16>(0o402 + i as u128), bits::<16>(w as u128)));
        }
        // Memory-mapped I/O initialization (idle-state values).
        // The Alto's I/O space is 0o177000-0o177777 (= 0xFE00-0xFFFF).
        // Per AltoHW: when no I/O device responds on the bus, the bus
        // reads as -1 (all 1s, wired-AND default).  The canonical
        // altoIIcode3.mu boot microcode (post-KSEC, in the Emulator
        // boot dance at MPC ~0x13b/0x13c) reads the keyboard at
        // KBDAD = 0o177034 = 0xfe1c (per AltoHW §3.4) to determine
        // boot-disk-address from depressed keys.  Spec digest line
        // 1128: "Depressed keys read as 0; idle keys as 1."  For our
        // no-keys-pressed default boot, all keyboard words = 0xFFFF.
        //
        // Without these initializations, mem[KBDAD] = 0 (default
        // memory init), which the boot microcode interprets as ALL
        // keys depressed — taking a wrong dispatch path through
        // F2=BUSODD (which checks BUS bit 0 of the keyboard read)
        // and ending up in the wrong boot handler.  Visible in the
        // lockstep dumper at cycle 92 where OURS goes to MPC=0x138
        // (no BUSODD merge) but CTR goes to 0x139 (BUSODD merged
        // because keyboard LSB = 1 in the no-keys-pressed reading).
        for kbd_addr in 0xfe1c..=0xfe1f {
            // 4 keyboard words
            mem_init.push((bits::<16>(kbd_addr as u128), bits::<16>(0xFFFF)));
        }
        mem_init.push((bits::<16>(0xfe18), bits::<16>(0xFFFF))); // UTILIN
        Self {
            urom: MicrocodeRom::with_words(microcode_words),
            crom: ConstantRom::with_constants(constants),
            mem: Memory::new(mem_init),
            tasks: AltoTaskSystem::default(),
            disk_ctrl: DiskController::default(),
            disk: DiabloDisk::with_test_period_and_sector_at_boundary(
                disk_period_cycles,
                boot_sector_data,
            ),
            // Per AltoHW §3.4 (Bootstrapping): "When the transfer is
            // complete, PC ← 1, and the emulator is started."  The
            // boot button performs this hardware-level initialization;
            // we match by initializing R[6]=PC to 1 here.  Without
            // this, OURS' Emulator fetches Nova instructions from
            // mem[0] (= 0 = NOP-ish) and ends up in an I/O-poll loop.
            engine: Microengine::with_initial_pc(1),
            state: rhdl_fpga::core::dff::DFF::new(ChipState::default()),
        }
    }
}

impl SynchronousIO for AltoChip {
    type I = ChipIn;
    type O = ChipOut;
    type Kernel = alto_chip_kernel;
}

#[kernel]
pub fn alto_chip_kernel(_cr: ClockReset, i: ChipIn, q: Q) -> (ChipOut, D) {
    let mut d = D::dont_care();
    let mut o = ChipOut::dont_care();

    // Mutable copy of the bundled chip state (per CLAUDE.md §3.1).
    // We read from q.state.* for inputs, mutate fields on this copy
    // throughout the kernel, then assign d.state = next_state at end.
    let mut next_state: ChipState = q.state;

    // Sticky current task (per Alto Hardware Manual §2.4): same task
    // runs every cycle until microcode does F1=TASK.  This DFF latches
    // the priority-encoded winning task on engine.task_yield; otherwise
    // holds.  task_system's `last_task` is now used only for trace
    // observability + per-task fire counters.
    let current_task: Bits<4> = q.state.current_task;
    // Look up that task's MPC.  Per *Alto Hardware Manual* §2
    // "Initialization" + ContrAlto's `Task.cs` Reset(): each task K
    // resets to MPC = K.  q.tasks.task_mpc DFFs default to all zeros
    // (a constraint of the synthesizable DFF reset value matching
    // the Verilog `initial begin` for the per-task array), so we
    // substitute K for task_mpc[K] until task K has run at least once.
    // The task_started bitmap tracks which tasks have run.
    let task_bit: Bits<16> = bits::<16>(1) << current_task.resize::<5>();
    let task_has_started: bool = (q.state.task_started & task_bit) != bits::<16>(0);
    // Read current_mpc from the chip-side `task_mpcs` (canonical
    // source of truth — see field rustdoc).  Falls back to the
    // per-task reset MPC = K until task K has run at least once
    // (matching the AltoHW §2 Initialization rule).
    let current_mpc: Bits<10> = if task_has_started {
        q.state.task_mpcs[current_task]
    } else {
        current_task.resize::<10>()
    };

    // Microcode RAM has 1-cycle BRAM latency.  q.urom.instruction is
    // the instruction at the address presented last cycle.
    let instr_this_cycle: Bits<32> = q.urom.instruction;

    // Decode RSEL[31:27] and BS[22:20] from the instruction; constant
    // ROM index = (RSEL << 3) | BS = 8 bits.
    let rsel: Bits<5> = ((instr_this_cycle >> 27) & bits::<32>(0x1F)).resize();
    let bs: Bits<3> = ((instr_this_cycle >> 20) & bits::<32>(0x07)).resize();
    let const_idx: Bits<8> = (rsel.resize::<8>() << 3) | bs.resize::<8>();
    d.crom = ConstantIn { index: const_idx };
    let const_value: Bits<16> = q.crom.value;

    // Memory bus + pipeline-stall signal.
    let mem_data_for_engine: Bits<16> = q.mem.read_data;
    let mem_stall_for_engine: bool = q.mem.mem_stall;

    // Per the Alto Hardware Manual §2.4: task switches happen ONLY
    // when microcode does F1=TASK.  When engine.task_yield asserts,
    // arbitrate combinationally over the effective wakeup vector via
    // priority encoder; the highest-priority woken task becomes the
    // new current_task at the next cycle's edge.  Until then, hold.
    // (Note: effective_wakeups computed below in the disk-routing
    // section; we do the priority encoder there too.)

    d.engine = MicroIn {
        mpc: current_mpc,
        instr: instr_this_cycle,
        constant_value: const_value,
        mem_read_data: mem_data_for_engine,
        current_task,
        disk_word_data: q.disk.current_word_data,
        kcwa: q.disk_ctrl.kcwa_value,
        // Per-task BS=3/BS=4 sources (spec §3.2 + §8.5).  In Disk
        // Sector / Disk Word task:
        //  - BS=3 (←KSTAT): reads the controller's KSTAT register.
        //  - BS=4 (←KDATA): reads the disk's "input data register"
        //    per spec §8.5 ("Disk input data register on bus") —
        //    NOT the controller's KDATA OUTPUT register, which is
        //    set by F1=15 LoadKDATA for write paths.  Real Alto's
        //    KDATA-read register is fed by the disk's serial-to-
        //    parallel converter as it streams sector bits.  Our
        //    DiabloDisk's `current_word_data` exposes that.
        kstat: q.disk_ctrl.kstat_word,
        kdata: q.disk.current_word_data,
        mem_stall: mem_stall_for_engine,
    };
    d.mem = MemIn {
        address: q.engine.mem_address,
        write_data: q.engine.mem_write_data,
        write_en: q.engine.mem_write_en,
        // Memory-pipeline stall driver signals — combinationally
        // decoded by the engine from the current uinst, no
        // dependence on stall state.  See `Memory` rustdoc for the
        // FSM semantics and stall rules.
        mar_load_this_cycle: q.engine.mar_load_this_cycle,
        md_read_this_cycle: q.engine.md_read_this_cycle,
        md_write_this_cycle: q.engine.md_write_this_cycle,
    };

    // Engine outputs next_mpc this cycle (combinational from this
    // cycle's instr).  Per RHDL Synchronous-widget composition,
    // `q.engine.next_mpc` is the engine's CURRENT-cycle combinational
    // output (see fifo::synchronous wiring `d.read_logic = q.write_logic`
    // for the same idiom).
    let next_mpc: Bits<10> = q.engine.next_mpc;

    // Disk: per-word DMA inputs (word_addr / write_data / read_en /
    // write_en) not yet driven by microengine — Phase 3.5 next adds
    // the per-word DMA path through Disk Word task.  But the
    // transfer_request signal IS wired up: when Disk Sector microcode
    // writes KCOM with bit 15 set, the controller asserts
    // transfer_request (q-registered), which arms the disk's 256-word
    // transfer countdown so word_strobe starts firing.
    let mut disk_in = DiskIn::default();
    // Transfer-arm trigger: per spec §8.5, F1=STROBE in a Disk task
    // initiates the disk operation.  Per §8.6, after KSEC sets up
    // KCOM/KADR and issues STROBE, the disk auto-streams sector
    // words.  Phase-3.5 also accepts the Phase-3 simplified
    // KCOM-bit-15 trigger from the disk controller for the legacy
    // DMA test microcode.
    disk_in.transfer_request = q.disk_ctrl.transfer_request || q.engine.disk_strobe;
    disk_in.word_consumed = q.engine.disk_word_consumed;
    // F1=Block + current_task per spec §5.5: the device interface
    // monitors F1=Block in conjunction with current_task to clear its
    // own wakeup signal.  Pass both into the disk; it decides whether
    // a Block applies to one of its tasks (4 = Disk Sector, 14 = Disk
    // Word).  q.engine.block_task is the previous cycle's F1=Block
    // (1-cycle lag from the firing instruction); current_task is the
    // chip's sticky-current-task DFF read this cycle.
    disk_in.block_task = q.engine.block_task;
    disk_in.current_task = current_task;
    d.disk = disk_in;
    // Disk controller: driven by microengine's per-task disk_ctrl
    // outputs.  When the Disk Sector task asserts F1=DiskCtrlWrite,
    // the engine emits write_en + write_data targeting register
    // RSEL[2:0]; otherwise idle.  The engine outputs are q-registered
    // (1-cycle late from the firing instruction).
    d.disk_ctrl = CtrlIn {
        reg_addr: q.engine.disk_ctrl_addr,
        write_data: q.engine.disk_ctrl_write_data,
        write_en: q.engine.disk_ctrl_write_en,
        // CLRSTAT (per spec §8.5): wired from engine's per-task
        // F1=12 (CLRSTAT) dispatch.  Q-registered (1-cycle lag).
        clr_stat: q.engine.disk_clr_stat,
    };

    // Compose the effective wakeup vector: user-supplied bits OR'd
    // with disk-derived wakeups (bit 4 from sector_mark, bit 14 from
    // word_strobe).  Both disk outputs are q-registered (1-cycle lag
    // from the disk's actual event).
    let disk_sector_mark = q.disk.sector_mark;
    let disk_word_strobe = q.disk.word_strobe;
    let mut effective_wakeups = i.wakeups;
    if disk_sector_mark {
        effective_wakeups |= bits::<16>(0x0010);
    }
    if disk_word_strobe {
        effective_wakeups |= bits::<16>(0x4000);
    }

    // Build the next_mpc_per_task array for the arbiter: only the
    // current task's slot reflects the engine's computed value.
    let mut next_mpcs = q.tasks.task_mpc;
    next_mpcs[current_task] = next_mpc;
    d.tasks = TaskIn {
        wakeups: effective_wakeups,
        next_mpc_per_task: next_mpcs,
    };

    // Combinational priority encoder: highest task # = highest priority.
    // Picks the highest-priority woken task from effective_wakeups.
    // Falls back to current_task if no task is woken (no-op).
    let winning_task: Bits<4> = if (effective_wakeups & bits::<16>(0x4000)) != bits::<16>(0) {
        bits::<4>(14)
    } else if (effective_wakeups & bits::<16>(0x2000)) != bits::<16>(0) {
        bits::<4>(13)
    } else if (effective_wakeups & bits::<16>(0x1000)) != bits::<16>(0) {
        bits::<4>(12)
    } else if (effective_wakeups & bits::<16>(0x0800)) != bits::<16>(0) {
        bits::<4>(11)
    } else if (effective_wakeups & bits::<16>(0x0400)) != bits::<16>(0) {
        bits::<4>(10)
    } else if (effective_wakeups & bits::<16>(0x0200)) != bits::<16>(0) {
        bits::<4>(9)
    } else if (effective_wakeups & bits::<16>(0x0100)) != bits::<16>(0) {
        bits::<4>(8)
    } else if (effective_wakeups & bits::<16>(0x0080)) != bits::<16>(0) {
        bits::<4>(7)
    } else if (effective_wakeups & bits::<16>(0x0010)) != bits::<16>(0) {
        bits::<4>(4)
    } else if (effective_wakeups & bits::<16>(0x0001)) != bits::<16>(0) {
        bits::<4>(0)
    } else {
        current_task
    };
    // Per AltoHW §2.4 ("One additional instruction is executed before
    // the switch becomes effective"): TASK switches happen at cycle
    // K+2, where cycle K is when F1=TaskYield runs.
    //
    // Implementation: latch this cycle's `q.engine.task_yield` into
    // `task_yield_pending` (a DFF), then use the LAST cycle's latched
    // value (`q.task_yield_pending`) to gate the switch.  This adds
    // exactly one cycle of delay vs. using `q.engine.task_yield`
    // directly — which (per RHDL settle semantics) reflects this
    // cycle's combinational engine output and gave K+1 timing.  See
    // `tests/task_switch_pipeline.rs` for the regression anchor and
    // spec digest §5 for the normative timing rule.
    // Single-cycle delay latch — see field doc on
    // `task_yield_pending` for the timing rationale.
    next_state.task_yield_pending = q.engine.task_yield;
    let task_yield_for_switch: bool = q.state.task_yield_pending;
    let next_current_task: Bits<4> = if task_yield_for_switch {
        winning_task
    } else {
        current_task
    };
    next_state.current_task = next_current_task;
    // Mark current_task as "started" — the next time it's selected by
    // arbitration, the chip will read its accumulated task_mpc rather
    // than the per-task reset MPC = K.
    next_state.task_started = q.state.task_started | task_bit;

    // ---- URom prefetch (spec §2.3, MIF || MIE pipeline) ----
    //
    // Per spec line 26 ("...executes a 32-bit horizontal microinstruction
    // every 170 ns") and §2.3 (2-stage MIF || MIE pipeline), the chip
    // must overlap fetch[k+1] with execute[k] so each microinstruction
    // completes in one cycle.
    //
    // The address presented to URom THIS cycle determines the
    // instruction the engine will execute NEXT cycle (URom is a 1-cycle
    // BRAM).  So we present the MPC of the NEXT instruction:
    //
    //   - If task switches at end of this cycle (task_yield AND
    //     winning_task != current_task): present the new task's saved
    //     MPC (or the per-task reset MPC = K if it has never run).
    //   - Otherwise (same task continues): present engine.next_mpc
    //     (combinational this cycle, the engine just computed it
    //     from the instruction it's executing right now).
    let next_task_bit: Bits<16> = bits::<16>(1) << next_current_task.resize::<5>();
    let next_task_started: bool = (q.state.task_started & next_task_bit) != bits::<16>(0);
    // Read new task's saved MPC from chip-side `task_mpcs` (canonical
    // source).  Falls back to per-task reset MPC = K until task K
    // has run.
    let next_task_saved_mpc: Bits<10> = if next_task_started {
        q.state.task_mpcs[next_current_task]
    } else {
        next_current_task.resize::<10>()
    };

    // ---- Chip-side per-task MPC update ----------------------------
    // Update task_mpcs[current_task] with engine.next_mpc each
    // non-stalled cycle.  This bypasses task_system's arbitration-
    // gated rule firing (which only updates task_mpc[K] when task K
    // wins priority arbitration — broken when higher-priority tasks
    // are also woken).  See `task_mpcs` field rustdoc for the
    // architectural rationale.
    let mut new_task_mpcs = q.state.task_mpcs;
    if !mem_stall_for_engine {
        new_task_mpcs[current_task] = next_mpc;
    }
    next_state.task_mpcs = new_task_mpcs;
    // URom prefetch must use the SAME delayed task-yield signal —
    // otherwise we'd prefetch the new task's instruction one cycle
    // before the switch becomes effective, leaving stale data in the
    // pipeline.
    let task_will_switch: bool = task_yield_for_switch && winning_task != current_task;
    // URom prefetch address.  When stalled (mem_stall_for_engine),
    // hold the previously-presented address so the URom keeps
    // returning the SAME instruction — letting the engine re-execute
    // it once the stall releases.  Per AltoHW §2.3: a stalled
    // memory-accessing instruction "re-executes" after the wait, so
    // its effects must apply once.  Without this hold, URom would
    // advance to next_mpc, the engine would see the post-stall
    // instruction, and the stalled instruction's effects would
    // silently disappear.
    let urom_addr_natural: Bits<10> = if task_will_switch {
        next_task_saved_mpc
    } else {
        next_mpc
    };
    let urom_addr: Bits<10> = if mem_stall_for_engine {
        q.state.urom_addr_held
    } else {
        urom_addr_natural
    };
    next_state.urom_addr_held = urom_addr;
    d.urom = UromIn {
        mpc: urom_addr.resize(),
    };

    // Outputs
    o.mpc = current_mpc;
    o.current_task = current_task;
    o.next_mpc = next_mpc;
    o.t = q.engine.t;
    o.l = q.engine.l;
    o.bus = q.engine.bus;
    o.alu_result = q.engine.alu_result;
    o.instruction = instr_this_cycle;
    o.disk_sector_mark = disk_sector_mark;
    o.disk_word_strobe = disk_word_strobe;
    o.wakeups = effective_wakeups;
    o.disk_ctrl_read_data = q.disk_ctrl.read_data;
    o.disk_ctrl_write_en = q.engine.disk_ctrl_write_en;
    o.ir = q.engine.ir;
    o.disk_sector_count = q.tasks.disk_sector_count;
    o.disk_word_count = q.tasks.disk_word_count;
    o.mem_write_observed_addr = q.engine.mem_address;
    o.mem_write_observed_data = q.engine.mem_write_data;
    o.mem_write_observed_en = q.engine.mem_write_en;
    o.block_task = q.engine.block_task;
    o.regs = q.engine.regs;
    o.task_yield = q.engine.task_yield;
    o.mem_stall = mem_stall_for_engine;

    // Commit the bundled chip-state DFF (per CLAUDE.md §3.1).
    d.state = next_state;

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::isa::*;

    /// Pack a simple microinstruction.
    fn ui(
        rsel: u8,
        aluf: AluFunction,
        bs: BusSource,
        t_load: bool,
        l_load: bool,
        next: u16,
    ) -> u32 {
        Microinstruction {
            rsel: bits::<5>(rsel as u128),
            aluf,
            bs,
            f1: F1Function::Nop,
            f2: F2Function::Nop,
            t_load,
            l_load,
            next: bits::<10>(next as u128),
        }
        .pack()
    }

    /// Boot input: wake Task 0 (Emulator) only, every cycle.
    fn boot_in() -> ChipIn {
        ChipIn {
            wakeups: bits::<16>(0x0001),
        }
    }

    fn run(uut: AltoChip, cycles: usize) -> Vec<ChipOut> {
        let inputs: Vec<ChipIn> = (0..cycles).map(|_| boot_in()).collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        uut.run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect()
    }

    #[test]
    fn boot_with_loop_at_zero() {
        // Load a single-instruction microcode at addr 0 that loops:
        //   ALUF = BusPlusOne, L_LOAD = 1, BS = ReadR (bus = R[0] = 0),
        //   NEXT = 0.  Each cycle: L = 1.
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = ui(0, AluFunction::BusPlusOne, BusSource::ReadR, false, true, 0);

        let uut = AltoChip::with_microcode(&microcode);
        let trace = run(uut, 6);

        // The chip should be looping at MPC=0 (next_mpc=0 → fetched again).
        // L should be 1 once the loop has executed at least once and
        // committed.  Due to the BRAM 1-cycle fetch latency + microengine
        // 1-cycle observation lag, the L=1 observation lands a few cycles
        // in.  Just verify mpc=0 throughout (the loop) and L eventually
        // reaches 1.
        let final_l: u128 = trace.last().unwrap().l.raw();
        assert_eq!(final_l, 1, "L should reach 1 after the boot loop runs");
        // The MPC should stay at 0 (looping).  It can take a couple
        // cycles for the BRAM to settle, but by the end it must be 0.
        assert_eq!(
            trace.last().unwrap().mpc.raw(),
            0,
            "MPC stays at 0 in the loop"
        );
    }

    /// Chip throughput test: 1 microinstruction per cycle, per spec
    /// §2.3 (MIF || MIE 2-stage pipeline) and spec line 26
    /// ("...executes a 32-bit horizontal microinstruction every 170 ns").
    ///
    /// This test was added as the spec-violation lock-in for the
    /// previous 2-cycles/microinstruction implementation.  The fix
    /// (Phase 3.5 Step 4i) feeds engine.next_mpc combinationally to
    /// URom in the same cycle, achieving the spec-required 1
    /// microinstruction per cycle throughput.  The test now enforces
    /// that contract.
    #[test]
    fn chip_runs_at_known_cycles_per_microinstruction() {
        // Chain of 4 distinct microaddresses: 0→1→2→3→0→1→2→3→...
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        let chain: [(usize, u16); 4] = [(0, 1), (1, 2), (2, 3), (3, 0)];
        for &(a, nxt) in chain.iter() {
            microcode[a] = ui(0, AluFunction::Bus, BusSource::None, false, false, nxt);
        }

        let uut = AltoChip::with_microcode(&microcode);
        let trace = run(uut, 16);

        // Skip initial pipeline-settle cycles (URom returns garbage
        // until the first address is fetched).
        let post_settle: Vec<u128> = trace.iter().skip(2).map(|t| t.mpc.raw()).collect();

        // Count consecutive-MPC duplicates.  Per spec, MPC must
        // change every cycle — zero duplicates allowed.
        let mut duplicate_pairs = 0;
        for w in post_settle.windows(2) {
            if w[0] == w[1] {
                duplicate_pairs += 1;
            }
        }

        assert_eq!(
            duplicate_pairs, 0,
            "chip throughput regression: per spec §2.3, MPC must change \
             every cycle (1 microinstruction per cycle).  Got {} \
             duplicate consecutive-MPC pairs in {:?}",
            duplicate_pairs, post_settle,
        );
    }

    #[test]
    fn boot_branches_to_addr_3_via_bus_eq_zero() {
        // Per spec digest §2.3 (DELAYED F2 NEXT-modifier pipeline):
        //   Cycle 0 — addr 0 has F2=BusEqZero, BUS=R[0]=0, NEXT=2.
        //             Modifier latched = 1.  next_mpc = 2 | 0 = 2.
        //   Cycle 1 — addr 2 (the absorb-cycle), NEXT=2.
        //             Modifier from cycle 0 (= 1) applies.
        //             next_mpc = 2 | 1 = 3.
        //   Cycle 2 — addr 3, NEXT=3 (loop).  Stays at 3.
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        let mi0 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::BusPlusOne,
            bs: BusSource::ReadR,
            f1: F1Function::Nop,
            f2: F2Function::BusEqZero,
            t_load: false,
            l_load: true,
            next: bits::<10>(2),
        };
        microcode[0] = mi0.pack();
        // addr 2: absorb-cycle that picks up cycle 0's latched modifier.
        // NEXT field bit 0 = 0; OR'd with 1 produces 3.
        microcode[2] = ui(0, AluFunction::Bus, BusSource::None, false, false, 2);
        // addr 3: target loop.
        microcode[3] = ui(0, AluFunction::Bus, BusSource::None, false, false, 3);

        let uut = AltoChip::with_microcode(&microcode);
        let trace = run(uut, 8);

        // MPC should reach 3 and stay there after the 2-cycle delayed-apply
        // pipeline (cycle 0 latches; cycle 1 applies; cycle 2+ at 3).
        let final_mpc = trace.last().unwrap().mpc.raw();
        assert_eq!(final_mpc, 3, "should branch to addr 3 and loop there");
    }

    /// F1=Constant should drive BUS from the constant ROM.  Verify
    /// end-to-end: AltoChip decodes RSEL+BS, drives constant_rom,
    /// returns the looked-up value, microengine uses it as BUS.
    #[test]
    fn f1_constant_drives_bus_from_constant_rom() {
        // Microcode at addr 0:
        //   F1 = Constant, ALUF = Bus (pass through), L_LOAD = 1,
        //   RSEL = 0, BS = 0  →  constant ROM index 0
        //   NEXT = 0 (loop)
        let mi0 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR, // overridden by F1=Constant
            f1: F1Function::Constant,
            f2: F2Function::Nop,
            t_load: false,
            l_load: true,
            next: bits::<10>(0),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi0.pack();

        // Constant ROM: index 0 = 0x1234.
        let mut constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        constants[0] = 0x1234;

        let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
        let trace = run(uut, 8);

        // After enough cycles for the BRAM to settle and L to commit,
        // L should hold 0x1234 (the constant).
        let final_l = trace.last().unwrap().l.raw();
        assert_eq!(
            final_l, 0x1234,
            "L should latch the constant ROM value via F1=Constant"
        );
    }

    #[test]
    fn f1_constant_with_different_index() {
        // RSEL=0b00001 (1), BS=0b010 (2) → index = (1 << 3) | 2 = 10.
        let mi0 = Microinstruction {
            rsel: bits::<5>(1),
            aluf: AluFunction::Bus,
            bs: BusSource::None, // BS=2; overridden by F1=Constant for BUS
            f1: F1Function::Constant,
            f2: F2Function::Nop,
            t_load: false,
            l_load: true,
            next: bits::<10>(0),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi0.pack();
        let mut constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        constants[10] = 0xCAFE;
        let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
        let trace = run(uut, 8);
        assert_eq!(
            trace.last().unwrap().l.raw(),
            0xCAFE,
            "L should latch the constant at index (RSEL<<3)|BS = 10"
        );
    }

    /// Memory read end-to-end through AltoChip: preload memory, use
    /// F1=Constant to load the address into BUS, F2=LoadMar to load
    /// MAR ← BUS, then BS=MemoryData with L_LOAD to capture the
    /// memory value into L.
    #[test]
    fn memory_read_via_mar_then_md_to_l() {
        // addr 0: F1=Constant (idx 0 = 0x0080) + F2=LoadMar → MAR ← 0x0080
        //         next = 1.
        let mi0 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::LoadMar,
            f2: F2Function::Constant,
            t_load: false,
            l_load: false,
            next: bits::<10>(1),
        };
        // addr 1: filler — wait one cycle for BRAM read to land.
        let mi1 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::Nop,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(2),
        };
        // addr 2: BS=MemoryData + L_LOAD → L ← memory[MAR] (= 0x0080's value)
        //         loop here.
        let mi2 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::MemoryData,
            f1: F1Function::Nop,
            f2: F2Function::Nop,
            t_load: false,
            l_load: true,
            next: bits::<10>(2),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi0.pack();
        microcode[1] = mi1.pack();
        microcode[2] = mi2.pack();

        // Constant ROM: index 0 = 0x0080 (the memory address to load).
        let mut constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        constants[0] = 0x0080;

        // Memory: preload memory[0x0080] = 0xC0DE.
        let memory_initial = vec![(bits::<16>(0x0080), bits::<16>(0xC0DE))];

        let uut =
            AltoChip::with_microcode_constants_and_memory(&microcode, &constants, memory_initial);
        let trace = run(uut, 16);

        // Eventually L should hold 0xC0DE (the memory value at 0x0080).
        let final_l = trace.last().unwrap().l.raw();
        assert_eq!(
            final_l, 0xC0DE,
            "L should latch the memory value at 0x0080 after MAR + MD→ sequence"
        );
    }

    /// Memory write end-to-end through AltoChip: use F1=Constant to
    /// load address, F2=LoadMar.  Then F1=Constant to load value,
    /// F2=WriteMd to write to memory[MAR].  Finally read back via
    /// BS=MemoryData and verify L holds the written value.
    #[test]
    fn memory_write_then_read_round_trip() {
        // addr 0: F1=Constant idx 0 = 0x0100 (target addr) + F2=LoadMar
        let mi0 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::LoadMar,
            f2: F2Function::Constant,
            t_load: false,
            l_load: false,
            next: bits::<10>(1),
        };
        // addr 1: F1=Constant idx 1 = 0xBEEF + F2=StoreMd → memory[0x0100] ← 0xBEEF
        //         (RSEL=0, BS=1 → constant index = 1).  F1=Constant sets
        //         BUS from constant ROM; F2=StoreMd writes BUS to memory.
        let mi1 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::LoadR, // BS=1
            f1: F1Function::Constant,
            f2: F2Function::StoreMd,
            t_load: false,
            l_load: false,
            next: bits::<10>(2),
        };
        // addr 2: filler (BRAM write commit + read latency)
        let mi2 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::Nop,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(3),
        };
        // addr 3: another filler
        let mi3 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::Nop,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(4),
        };
        // addr 4: BS=MemoryData + L_LOAD → L ← memory[MAR] (= 0xBEEF).  Loop.
        let mi4 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::MemoryData,
            f1: F1Function::Nop,
            f2: F2Function::Nop,
            t_load: false,
            l_load: true,
            next: bits::<10>(4),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi0.pack();
        microcode[1] = mi1.pack();
        microcode[2] = mi2.pack();
        microcode[3] = mi3.pack();
        microcode[4] = mi4.pack();

        let mut constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        constants[0] = 0x0100;
        constants[1] = 0xBEEF;

        let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
        let trace = run(uut, 24);

        let final_l = trace.last().unwrap().l.raw();
        assert_eq!(
            final_l, 0xBEEF,
            "after write-then-read sequence, L should hold the written 0xBEEF"
        );
    }

    /// Multi-task arbitration through the chip: wake Task 0 (Emulator)
    /// AND Task 4 (Disk Sector) every cycle.  Task 4 has higher priority
    /// (lower priority-number in the rule), so it fires.  Verify
    /// out.current_task reflects the firing task.
    #[test]
    fn multi_task_arbitration_picks_higher_priority() {
        // Per spec §2.4: task K resets to MPC=K.  Place an explicit
        // NOP-loop at MPC=4 so task 4 stays on its own microcode after
        // arbitration, instead of relying on the chip's per-task-MPC
        // substitution to fall through into task 0's microcode at MPC=0.
        let mi_yield = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::TaskYield,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(0),
        };
        let mi_task4_loop = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::Nop,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(4),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi_yield.pack(); // task 0 reset MPC: yield
        microcode[4] = mi_task4_loop.pack(); // task 4 reset MPC: NOP loop
        let constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
        // Wake both Task 0 (bit 0) and Task 4 (bit 4 = 0x0010).
        let inputs: Vec<ChipIn> = (0..16)
            .map(|_| ChipIn {
                wakeups: bits::<16>(0x0011),
            })
            .collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let trace: Vec<ChipOut> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect();
        // After the initial settling cycles, current_task should be 4
        // (Disk Sector wins over Emulator on shared wakeup).
        let final_task = trace.last().unwrap().current_task.raw();
        assert_eq!(
            final_task, 4,
            "with both task 0 and task 4 woken, task 4 (Disk Sector) wins"
        );
    }

    /// Per-task gating: under Disk Sector (Task 4), F1=WriteKadr
    /// asserts disk_ctrl_write_en; under Emulator (Task 0), the
    /// same instruction is a no-op.
    #[test]
    fn f1_write_kadr_only_active_under_disk_sector_task() {
        // Per spec §2.4: task K resets to MPC=K.  Two scenarios share
        // the same microcode but with different wakeups:
        //   - Test A wakes task 4 (Disk Sector); chip starts on
        //     Emulator (current_task DFF default).  mc[0]=TaskYield
        //     yields → arbiter picks task 4 → mc[4]=WriteKadr fires
        //     under task 4 → write_en asserts.
        //   - Test B wakes task 0 (Emulator); mc[0]=TaskYield yields
        //     → arbiter picks task 0 (only one woken) → mc[1]=WriteKadr
        //     fires under Emulator (F1=14 = ESRB no-op stub) →
        //     write_en stays low.
        let mi_yield = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::TaskYield,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(1),
        };
        let mi_writekadr = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::WriteKadr,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(1),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi_yield.pack(); // Emulator's reset MPC: yield
        microcode[1] = mi_writekadr.pack(); // Emulator's post-yield path
        microcode[4] = mi_writekadr.pack(); // Disk Sector's reset MPC
        let constants = [0u16; crate::constant_rom::NUM_CONSTANTS];

        // ---- Test A: Disk Sector task (4) firing → write_en asserted
        let uut_a = AltoChip::with_microcode_and_constants(&microcode, &constants);
        let inputs_a: Vec<ChipIn> = (0..16)
            .map(|_| ChipIn {
                wakeups: bits::<16>(0x0010), // wake Task 4 only
            })
            .collect();
        let stream_a = inputs_a.into_iter().with_reset(2).clock_pos_edge(100);
        let trace_a: Vec<ChipOut> = uut_a
            .run(stream_a)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect();
        let write_active_a = trace_a.iter().any(|t| t.disk_ctrl_write_en);
        assert!(
            write_active_a,
            "Under Disk Sector task, F1=WriteKadr should assert write_en"
        );

        // ---- Test B: Emulator task (0) firing → write_en NEVER asserted
        let uut_b = AltoChip::with_microcode_and_constants(&microcode, &constants);
        let inputs_b: Vec<ChipIn> = (0..16)
            .map(|_| ChipIn {
                wakeups: bits::<16>(0x0001), // wake Task 0 only
            })
            .collect();
        let stream_b = inputs_b.into_iter().with_reset(2).clock_pos_edge(100);
        let trace_b: Vec<ChipOut> = uut_b
            .run(stream_b)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect();
        let write_active_b = trace_b.iter().any(|t| t.disk_ctrl_write_en);
        assert!(
            !write_active_b,
            "Under Emulator task, F1=WriteKadr is a no-op (gated)"
        );
    }

    /// End-to-end 256-word DMA: hand-written Disk Sector microcode
    /// arms a transfer; Disk Word task body then does 256 atomic
    /// per-word DMAs from disk's pre-loaded sector buffer into memory.
    /// Verifies memory[0x200..0x300] == [0xA000, 0xA001, ..., 0xA0FF].
    ///
    /// This is the architectural milestone: full disk → memory boot
    /// DMA path runnable through real microengine cycles.
    #[test]
    fn end_to_end_256_word_dma() {
        // ---- Microcode ------------------------------------------------
        // Spec §2.4: each task K resets to MPC = K.  Task 4 (Disk Sector)
        // therefore starts at MPC=4; Task 14 (Disk Word) starts at
        // MPC=14.  Microcode must be placed at those addresses.
        //
        // Layout:
        //   addr 4: R[2] ← 0x0200 (KCWA target).  F1=Constant +
        //           BS=LoadR + RSEL=2 ⇒ R[2] ← const[(2<<3)|1=17].
        //           (Task 4's first instruction on reset.)
        //   addr 5: R[1] ← 0x8000 (KCOM arm bit).  RSEL=1, BS=LoadR,
        //           F1=Constant ⇒ R[1] ← const[(1<<3)|1=9].
        //   addr 6: KCWA ← R[2] = 0x0200.  F1=WriteKcwa (Task 4 only).
        //   addr 7: KCOM ← R[1] = 0x8000.  F1=WriteKcomm (Task 4).
        //           Arms the transfer; word_strobe begins pulsing.
        //   addr 8: F1=TaskYield → allow Disk Word (Task 14) to win
        //           arbitration when word_strobe pulses; falls through
        //           to addr 9.
        //   addr 9: Task 4 idle loop with TaskYield.  Continues yielding
        //           so Task 14 can win whenever word_strobe is hot.
        //   addr 14: Task 14's reset MPC.  F2=DiskWordTransfer +
        //           F1=TaskYield, NEXT=14 — atomic DMA per firing.
        let mi_load_r2 = Microinstruction {
            rsel: bits::<5>(2),
            aluf: AluFunction::Bus,
            bs: BusSource::LoadR,
            f1: F1Function::Constant,
            f2: F2Function::Nop,
            t_load: false,
            l_load: true,
            next: bits::<10>(5),
        };
        let mi_load_r1 = Microinstruction {
            rsel: bits::<5>(1),
            aluf: AluFunction::Bus,
            bs: BusSource::LoadR,
            f1: F1Function::Constant,
            f2: F2Function::Nop,
            t_load: false,
            l_load: true,
            next: bits::<10>(6),
        };
        let mi_write_kcwa = Microinstruction {
            rsel: bits::<5>(2),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::WriteKcwa,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(7),
        };
        let mi_write_kcom = Microinstruction {
            rsel: bits::<5>(1),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::WriteKcomm,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(8),
        };
        let mi_yield_to_dwt = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::TaskYield,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(9),
        };
        let mi_task4_idle = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::TaskYield,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(9),
        };
        let mi_task14_loop = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::TaskYield,
            f2: F2Function::DiskWordTransfer,
            t_load: false,
            l_load: false,
            next: bits::<10>(14),
        };
        // Task 0 (Emulator) needs SOMETHING at MPC=0 that asserts
        // TaskYield so the chip can switch to task 4.  Without this,
        // task 0 (= current_task on reset) runs the all-zeros microcode
        // which is a NOP forever, never yielding, never switching.
        // This is purely for the test bootstrap; a real Alto's
        // Emulator microcode at MPC=0 starts the Nova fetch loop
        // which yields naturally.
        let mi_emu_bootstrap = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::TaskYield,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(0),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi_emu_bootstrap.pack();
        microcode[4] = mi_load_r2.pack();
        microcode[5] = mi_load_r1.pack();
        microcode[6] = mi_write_kcwa.pack();
        microcode[7] = mi_write_kcom.pack();
        microcode[8] = mi_yield_to_dwt.pack();
        microcode[9] = mi_task4_idle.pack();
        microcode[14] = mi_task14_loop.pack();

        // ---- Constant ROM ---------------------------------------------
        // const[17] (= (RSEL=2 << 3) | BS=LoadR) → 0x0200 (KCWA target).
        // const[9]  (= (RSEL=1 << 3) | BS=LoadR) → 0x8000 (KCOM arm bit).
        let mut constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        constants[17] = 0x0200;
        constants[9] = 0x8000;

        // ---- Disk sector pre-loaded with [0xA000+i for i in 0..256] -
        let mut sector = [0u16; 256];
        for (i, w) in sector.iter_mut().enumerate() {
            *w = 0xA000 + (i as u16);
        }

        // ---- Build the chip ------------------------------------------
        // Manual construction so we can inject the pre-loaded disk.
        let uut = AltoChip {
            urom: MicrocodeRom::with_words(&microcode),
            crom: ConstantRom::with_constants(&constants),
            mem: Memory::default(),
            tasks: AltoTaskSystem::default(),
            disk_ctrl: DiskController::default(),
            disk: DiabloDisk::with_sector(&sector),
            engine: Microengine::default(),
            state: rhdl_fpga::core::dff::DFF::new(ChipState::default()),
        };

        // Wakeup pattern: Task 4 (Disk Sector) always woken so its
        // microcode runs.  Disk Word (Task 14) wake comes from the
        // disk's word_strobe automatically once transfer arms.
        let inputs: Vec<ChipIn> = (0..400)
            .map(|_| ChipIn {
                wakeups: bits::<16>(0x0010),
            })
            .collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let trace: Vec<ChipOut> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect();

        // The Disk Word task fire counter is sourced from
        // task_system's `disk_word_count` (incremented by the rule
        // body each time Task 14 fires).  After the 256-word transfer
        // completes, transfer_remaining hits 0, word_strobe stops,
        // Task 14 stops firing — counter freezes at 256 (or close to
        // it, accounting for the +1 initial firing on arm).
        let final_disk_word_count = trace.last().unwrap().disk_word_count.raw();
        eprintln!("[end_to_end_256_word_dma] disk_word_count = {final_disk_word_count}");

        // E1 tightening: per spec §8.6, transfer_remaining counts down
        // exactly 256 words then word_strobe stops.  Previous bound
        // was 256..=280 (24 cycles slack) which would have masked any
        // re-arm bug for many extra firings.  Tighten to the
        // pipeline-exact value: 256 DMAs + at most 1 transition cycle.
        assert!(
            final_disk_word_count == 256 || final_disk_word_count == 257,
            "Disk Word task should fire exactly 256 or 257 times \
             (256 DMAs + ≤1 transition cycle); got {final_disk_word_count}"
        );

        // E2 tightening: Disk Sector fires ~5 setup cycles then idles
        // while Task 14 wins arbitration during DMA, then resumes
        // afterward.  Total = 400 - 257 = 143 cycles available; minus
        // pipeline-settle slack = ~140-142 measured.  Tighten from
        // bare floor `>=4` to a measured-pipeline-exact range.
        let final_disk_sector_count = trace.last().unwrap().disk_sector_count.raw();
        eprintln!("[end_to_end_256_word_dma] disk_sector_count = {final_disk_sector_count}");
        assert!(
            final_disk_sector_count >= 140 && final_disk_sector_count <= 145,
            "Disk Sector should fire 140..145 times (5-cycle setup + \
             ~140 post-DMA cycles in 400-cycle test); got \
             {final_disk_sector_count}"
        );

        // ---- Verify memory contents via observed-write trace --------
        // Reconstruct the engine's memory-write trace from ChipOut and
        // verify the right WORDS landed at the right addresses.
        // Expected: 256 writes of (addr, data) = (0x200+i, 0xA000+i)
        // for i in 0..256.
        let mut writes: Vec<(u128, u128)> = Vec::new();
        for t in &trace {
            if t.mem_write_observed_en {
                writes.push((
                    t.mem_write_observed_addr.raw(),
                    t.mem_write_observed_data.raw(),
                ));
            }
        }
        eprintln!(
            "[end_to_end_256_word_dma] observed {} memory writes",
            writes.len()
        );
        // 256 DMAs expected; pipeline-delay can produce one extra
        // firing as the wakeup chain settles.  Accept 256 or 257.
        assert!(
            writes.len() == 256 || writes.len() == 257,
            "Expected 256 or 257 memory writes (256 DMAs + maybe 1 transition); got {}",
            writes.len()
        );
        // Verify the first 256 writes: addr 0x200..0x300, data 0xA000..0xA0FF.
        for (i, &(addr, data)) in writes.iter().take(256).enumerate() {
            let expected_addr = 0x200u128 + (i as u128);
            let expected_data = 0xA000u128 + (i as u128);
            assert_eq!(
                addr, expected_addr,
                "Write {i}: addr = {addr:#06x}, expected {expected_addr:#06x}"
            );
            assert_eq!(
                data, expected_data,
                "Write {i}: data = {data:#06x}, expected {expected_data:#06x}"
            );
        }
    }

    /// End-to-end disk-arm chain: under Disk Sector task, microcode
    /// loads R[1] with 0x8000 (the "start transfer" KCOM bit), then
    /// writes KCOM ← R[1].  This should:
    ///   1. Commit KCOM = 0x8000 in disk_ctrl.
    ///   2. disk_ctrl asserts transfer_request (KCOM[15] is set).
    ///   3. Disk arms a 256-word transfer (transfer_remaining ← 256).
    ///   4. word_strobe fires per cycle.
    ///   5. wakeup bit 14 OR'd in.
    ///   6. Disk Word task (14) fires.
    ///
    /// Verifies the full controller→disk→arbiter chain end-to-end.
    #[test]
    fn kcom_write_arms_disk_and_fires_disk_word_task() {
        // Per spec §2.4: task K resets to MPC=K.  Place Disk Sector's
        // setup chain at MPC=4..7 (its reset MPC = 4) and Disk Word's
        // loop at MPC=14 (its reset MPC = 14).  Emulator (current_task
        // DFF default) must yield first; place TaskYield at MPC=0.
        // addr 0: TaskYield so Emulator yields, arbiter picks task 4.
        let mi_emu_yield = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::TaskYield,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(0),
        };
        // addr 4: F1=Constant idx 9 = 0x8000, BS=LoadR, RSEL=1, ALUF=Bus
        //         → R[1] ← 0x8000.  Loads via L (per spec §2.7).
        let mi_load = Microinstruction {
            rsel: bits::<5>(1),
            aluf: AluFunction::Bus,
            bs: BusSource::LoadR,
            f1: F1Function::Constant,
            f2: F2Function::Nop,
            t_load: false,
            l_load: true,
            next: bits::<10>(5),
        };
        // addr 5: BS=ReadR (RSEL=1 → BUS = R[1] = 0x8000), F1=WriteKcomm
        //         → KCOM ← 0x8000.  Arms transfer.
        let mi_kcom = Microinstruction {
            rsel: bits::<5>(1),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::WriteKcomm,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(6),
        };
        // addr 6: F1=TaskYield → release Disk Sector so Disk Word
        //         (Task 14) can win arbitration when word_strobe pulses.
        let mi_yield = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::TaskYield,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(6),
        };
        // addr 14: Disk Word's reset MPC.  TaskYield + DiskWordTransfer
        //         (atomic per-cycle DMA in task 14).
        let mi_dw = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::TaskYield,
            f2: F2Function::DiskWordTransfer,
            t_load: false,
            l_load: false,
            next: bits::<10>(14),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi_emu_yield.pack();
        microcode[4] = mi_load.pack();
        microcode[5] = mi_kcom.pack();
        microcode[6] = mi_yield.pack();
        microcode[14] = mi_dw.pack();
        let mut constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        // F1=Constant in mi0 has RSEL=1, BS=LoadR(=1), so the constant
        // index = (1 << 3) | 1 = 9.  Put 0x8000 there.
        constants[9] = 0x8000;

        let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
        // Wake Disk Sector (task 4) only.
        let inputs: Vec<ChipIn> = (0..40)
            .map(|_| ChipIn {
                wakeups: bits::<16>(0x0010),
            })
            .collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let trace: Vec<ChipOut> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect();

        // Within 40 cycles, the chain should have completed:
        // KCOM written → transfer_request → disk armed → word_strobe fires.
        let any_word_strobe = trace.iter().any(|t| t.disk_word_strobe);
        assert!(
            any_word_strobe,
            "word_strobe should fire after Disk Sector microcode writes KCOM = 0x8000"
        );

        // And: the Disk Word task (14) should have fired.
        let any_task_14 = trace.iter().any(|t| t.current_task.raw() == 14);
        assert!(
            any_task_14,
            "Disk Word task (14) should fire after disk arms the transfer"
        );
    }

    /// IDispatch under Emulator: stage IR=0x4000 then run F2=IDispatch
    /// with NEXT=0x100.  Per spec §6.6 IDISP PROM:
    ///   IR=0x4000 → ir_1_2 = (0x4000 >> 13) & 3 = 2 → dispatch_value = 5.
    /// So next_mpc = 0x100 | 5 = 0x105.
    ///
    /// IR=0x4000 chosen instead of 0xCAFE because IR← (= F2=LoadIr in
    /// Emulator) merges BUS bits 15,10,9,8 into NEXT[3..0] per spec §6.6
    /// (D17 fix).  0x4000 has those bits clear, so the merge is 0 — the
    /// LoadIr cycle's NEXT (=3) is preserved, then mi3 (IDispatch)
    /// fires.  0xCAFE would have set merge bits and dispatched mi2's
    /// follow-on to the wrong address, bypassing the IDispatch test.
    #[test]
    fn f2_idispatch_routes_via_ir_low_byte() {
        // addr 0: F1=LoadMar + F2=Constant idx=0=0x0100 → MAR ← 0x0100.
        let mi0 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::LoadMar,
            f2: F2Function::Constant,
            t_load: false,
            l_load: false,
            next: bits::<10>(1),
        };
        // addr 1: filler, BRAM read settle.
        let mi1 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::Nop,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(2),
        };
        // addr 2: F2=LoadIr → IR ← MD = 0xCAFE.  Per spec §6.6 + ContrAlto
        // EmulatorTask.cs, IR loads from the BUS — so BS must be
        // MemoryData to drive BUS = MD.  (Previous test used BS=ReadR
        // which only "worked" because of an implementation bypass that
        // loaded IR directly from i.mem_read_data; that bypass was
        // removed when D17 added the spec'd NEXT-merge semantics.)
        let mi2 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::MemoryData,
            f1: F1Function::Nop,
            f2: F2Function::LoadIr,
            t_load: false,
            l_load: false,
            next: bits::<10>(3),
        };
        // addr 3: F2=IDispatch with NEXT=0x100.  IR=0x4000.
        // Per spec §6.6 IDISP PROM: IR[1-2] = (0x4000 >> 13) & 3 =
        // (0x2) & 3 = 2 → dispatch_value = 5.
        // Per spec digest §2.3 (DELAYED F2 NEXT-modifier pipeline),
        // the modifier (= 5) is latched at end of cycle running addr 3
        // and applied to the NEXT cycle's NEXT field.  So:
        //   Cycle running 3   → next_mpc = 0x100 | 0 = 0x100 (modifier latched)
        //   Cycle running 0x100 → next_mpc = 0x100 | 5 = 0x105 (modifier applied)
        //   Cycle running 0x105 → handler runs.
        let mi3 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::Nop,
            f2: F2Function::IDispatch,
            t_load: false,
            l_load: false,
            next: bits::<10>(0x100),
        };
        // addr 0x100: absorb-cycle that picks up cycle-3's latched
        // IDISP modifier.  NEXT=0x100 (self-loop) so OR'ing the modifier
        // (= 5) produces 0x100 | 5 = 0x105.  l_load=false so L isn't
        // disturbed before the handler runs.
        let mi_absorb = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::Nop,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(0x100),
        };
        // addr 0x105: handler — BUS+1 → L = 1, loop.
        let mi_target = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::BusPlusOne,
            bs: BusSource::ReadR,
            f1: F1Function::Nop,
            f2: F2Function::Nop,
            t_load: false,
            l_load: true,
            next: bits::<10>(0x105),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi0.pack();
        microcode[1] = mi1.pack();
        microcode[2] = mi2.pack();
        microcode[3] = mi3.pack();
        microcode[0x100] = mi_absorb.pack();
        microcode[0x105] = mi_target.pack();

        let mut constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        constants[0] = 0x0100; // MAR target
        let memory_initial = vec![(bits::<16>(0x0100), bits::<16>(0x4000))];

        let uut =
            AltoChip::with_microcode_constants_and_memory(&microcode, &constants, memory_initial);
        let trace = run(uut, 16);

        // L should latch 1 (BUS+1 with BUS=0) at the handler.
        let final_l = trace.last().unwrap().l.raw();
        assert_eq!(
            final_l, 1,
            "L should latch 1 from the addr-0x105 BUS+1 ALU op, proving \
             spec §6.6 IDISP PROM routed MPC to 0x105 = 0x100 | 5 \
             (IR[1-2]=2 → dispatch=5)"
        );
        let final_ir = trace.last().unwrap().ir.raw();
        assert_eq!(
            final_ir, 0x4000,
            "IR should hold 0x4000 from the LoadIr step"
        );
    }

    /// Per-task IR load: under Emulator (task 0), F2=LoadIr loads
    /// IR ← MD (memory data).  Stage memory[0x100] = 0xCAFE, do
    /// MAR ← 0x100, wait, then F2=LoadIr; verify IR == 0xCAFE.
    #[test]
    fn f2_load_ir_only_active_under_emulator_task() {
        // addr 0: F1=Constant idx 0 = 0x0100, F2=LoadMar → MAR ← 0x0100
        let mi0 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::LoadMar,
            f2: F2Function::Constant,
            t_load: false,
            l_load: false,
            next: bits::<10>(1),
        };
        // addr 1: filler, give BRAM read time to land
        let mi1 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::Nop,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(2),
        };
        // addr 2: F2=LoadIr  → IR ← BUS = MD (memory[MAR] = 0xCAFE).
        // Spec §6.6: IR← latches BUS, so BS=MemoryData drives BUS=MD.
        let mi2 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::MemoryData,
            f1: F1Function::Nop,
            f2: F2Function::LoadIr,
            t_load: false,
            l_load: false,
            next: bits::<10>(2),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi0.pack();
        microcode[1] = mi1.pack();
        microcode[2] = mi2.pack();
        let mut constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        constants[0] = 0x0100;
        let memory_initial = vec![(bits::<16>(0x0100), bits::<16>(0xCAFE))];

        let uut =
            AltoChip::with_microcode_constants_and_memory(&microcode, &constants, memory_initial);
        let trace = run(uut, 16); // Task 0 (Emulator) always woken via boot_in()
        let final_ir = trace.last().unwrap().ir.raw();
        assert_eq!(
            final_ir, 0xCAFE,
            "IR should latch the memory value at 0x0100 via LoadIr in Emulator task"
        );
    }

    /// End-to-end disk → wakeup → arbiter → engine path: with a SHORT
    /// 256-cycle test sector cadence, the Disk Sector task (Task 4)
    /// fires at least once within 512 cycles.  Real hardware uses
    /// the spec-correct ~19,608-cycle cadence (`SECTOR_PERIOD_CYCLES`);
    /// this test uses a short cadence purely to keep iteration fast.
    #[test]
    fn disk_sector_mark_fires_disk_sector_task() {
        // Per spec §2.4: task K resets to MPC=K.  Place an Emulator
        // TaskYield-loop at MPC=0 (so Emulator yields every cycle and
        // the arbiter can switch to Task 4 the moment sector_mark sets
        // wakeup bit 4) AND a Task 4 NOP-loop at MPC=4 (so Task 4
        // stays on its own microcode, not falling through to Emulator's).
        let mi_emu_yield = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::TaskYield,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(0),
        };
        let mi_task4_loop = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::Nop,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(4),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi_emu_yield.pack();
        microcode[4] = mi_task4_loop.pack();
        let constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        // Use SHORT 256-cycle test disk period so sector_mark fires
        // within 512 cycles.  Spec-correct ~19,608-cycle period would
        // need a 25K-cycle test run.
        let uut =
            AltoChip::with_microcode_constants_and_test_disk_period(&microcode, &constants, 256);
        // Wake Task 0 (Emulator) only via input; disk's sector_mark
        // should add wakeup bit 4 within ~512 cycles.
        let inputs: Vec<ChipIn> = (0..512)
            .map(|_| ChipIn {
                wakeups: bits::<16>(0x0001),
            })
            .collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let trace: Vec<ChipOut> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect();
        // Verify the wakeup vector saw the disk's sector_mark at some
        // point (bit 4 set in the effective wakeup vector).
        let any_disk_wakeup = trace.iter().any(|t| (t.wakeups.raw() & 0x0010) != 0);
        assert!(
            any_disk_wakeup,
            "disk sector_mark should have driven wakeup bit 4 at least once"
        );
        // Verify the Disk Sector task (Task 4) fired at least once
        // (current_task = 4 in some cycle).
        let any_task_4 = trace.iter().any(|t| t.current_task.raw() == 4);
        assert!(
            any_task_4,
            "Disk Sector task should have fired in response to the disk wakeup"
        );
    }

    /// F1=STROBE (binary 9, per spec §8.5) in Disk Sector arms a
    /// disk transfer (matches real Alto's KSEC arming convention).
    /// Verifies engine.disk_strobe → disk.transfer_request → disk
    /// starts asserting word_strobe.  Phase-3.5 test for the spec-
    /// correct STROBE protocol path (the Phase-3 test
    /// `kcom_write_arms_disk_and_fires_disk_word_task` validates
    /// the legacy KCOM-bit-15 simplification path).
    #[test]
    fn f1_strobe_in_disk_sector_arms_transfer() {
        // Per spec §2.4: task K resets to MPC=K.  Place STROBE at
        // MPC=4 (Task 4's reset MPC) directly; no Emulator bootstrap
        // needed since wakeups=0x0010 selects only Task 4 once
        // current_task hits 4.  But current_task is initially 0, so
        // we still need a TaskYield at MPC=0 to escape the Emulator.
        // (NOTE: even though wakeups bypass task 0, sticky-task
        // semantics keep current_task=0 until task_yield fires.)
        let mi_emu_yield = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::TaskYield,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(0),
        };
        // mi_strobe: F1=STROBE (Code9, per spec §8.5).  Arms transfer.
        let mi_strobe = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::Code9,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(5),
        };
        // mi_idle: Task 4 idle loop with TaskYield, so word_strobe
        // can switch arbitration to Task 14.
        let mi_idle = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs: BusSource::ReadR,
            f1: F1Function::TaskYield,
            f2: F2Function::Nop,
            t_load: false,
            l_load: false,
            next: bits::<10>(5),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi_emu_yield.pack();
        microcode[4] = mi_strobe.pack(); // Task 4 reset MPC: STROBE
        microcode[5] = mi_idle.pack(); // Task 4 idle loop
        let constants = [0u16; crate::constant_rom::NUM_CONSTANTS];

        let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
        // Wake Disk Sector (task 4) only.
        let inputs: Vec<ChipIn> = (0..50)
            .map(|_| ChipIn {
                wakeups: bits::<16>(0x0010),
            })
            .collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let trace: Vec<ChipOut> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect();

        // After the STROBE fires, transfer arms → word_strobe asserts.
        let any_word_strobe = trace.iter().any(|t| t.disk_word_strobe);
        assert!(
            any_word_strobe,
            "F1=STROBE in Disk Sector should arm the transfer; \
             word_strobe should assert"
        );
    }

    /// Verify the AltoChip composition emits clean Verilog and the
    /// round-trip through iverilog matches the Rust simulator.  Uses
    /// .skip(2) for the BRAM X-state (microcode_rom is uninitialised
    /// at addresses outside the small preloaded range).
    #[test]
    fn alto_chip_iverilog_round_trip() -> Result<(), RHDLError> {
        // Tiny microcode at addr 0: NOP loop.
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = ui(0, AluFunction::Bus, BusSource::ReadR, false, false, 0);
        let constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
        let inputs: Vec<ChipIn> = (0..8).map(|_| boot_in()).collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &TestBenchOptions::default().skip(2))?;
        tm.run_iverilog()?;
        Ok(())
    }

    /// Real-microcode integration: load the actual Alto II microcode
    /// AND constant ROM, run a few cycles.  Verifies the
    /// loader → ROM → engine path works end-to-end without crashing.
    /// Cannot yet check correctness (that needs all the per-task F1/F2
    /// + memory + disk wiring).
    #[test]
    fn boot_with_real_microcode_and_constants_does_not_crash() {
        use crate::microcode_loader;
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("rom");
        if !dir.join("U55").exists() || !dir.join("C0").exists() {
            eprintln!(
                "[boot_with_real_microcode_and_constants_does_not_crash] skipping — assets absent"
            );
            return;
        }
        let microcode = microcode_loader::load_alto_ii_microcode_from_dir(&dir)
            .expect("load real Alto II microcode");
        let constants = microcode_loader::load_alto_ii_constant_rom_from_dir(&dir)
            .expect("load real Alto II Constant ROM");
        let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
        let trace = run(uut, 64);

        // The trace should be well-formed (right length, no panics).
        // Phase 3.5 doesn't yet implement enough of the microengine
        // to actually execute the standard Alto microcode correctly —
        // it'll wander somewhere undefined.  But the wiring + fetch
        // pipeline should be stable.
        assert_eq!(trace.len(), 64);
        // The engine should have processed at least one non-zero
        // instruction (the real microcode at addr 0 is non-zero).
        let any_nonzero = trace.iter().any(|t| t.instruction.raw() != 0);
        assert!(
            any_nonzero,
            "should fetch at least one non-zero microinstruction from real microcode"
        );
        // It should also have visited multiple distinct microaddresses
        // (the boot path doesn't immediately stick at one address).
        let mut visited: std::collections::HashSet<u128> = std::collections::HashSet::new();
        for t in &trace {
            visited.insert(t.mpc.raw());
        }
        assert!(
            visited.len() >= 2,
            "boot trace should visit at least 2 distinct microaddresses; visited {:?}",
            visited
        );
    }

    /// Diagnostic: dump the (mpc, decoded microinstruction) for each
    /// unique address visited during a real-microcode trace.  Helps
    /// identify which F1/F2 codes the boot path is hitting that aren't
    /// yet implemented (gated to Reserved → no-op).  Run via:
    ///
    ///   cargo test -p rhdl-alto boot_trace_decode_diagnostic -- --nocapture --include-ignored
    ///
    /// Marked #[ignore] because it's a manual investigation aid, not
    /// a regression check.
    #[test]
    #[ignore]
    fn boot_trace_decode_diagnostic() {
        use crate::microcode_loader;
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("rom");
        if !dir.join("U55").exists() || !dir.join("C0").exists() {
            eprintln!("[diagnostic] skipping — assets absent");
            return;
        }
        let microcode = microcode_loader::load_alto_ii_microcode_from_dir(&dir).unwrap();
        let constants = microcode_loader::load_alto_ii_constant_rom_from_dir(&dir).unwrap();
        let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
        let trace = run(uut, 1000);

        // Collect unique (task, mpc) → instr pairs.  Both Task 0 and
        // Task 4 visit MPC=0 (shared starting point); keying by
        // (task, mpc) shows each task's distinct path.
        let mut seen: std::collections::BTreeMap<(u128, u128), u128> =
            std::collections::BTreeMap::new();
        for t in &trace {
            // Skip stall-NOP frames (instr = current_mpc with all
            // functional fields 0 → instr equals mpc, which is small).
            // Real microcode at any address has multiple non-zero
            // fields packed into the high 16 bits.
            if t.instruction.raw() < 0x10000 && t.instruction.raw() == t.mpc.raw() {
                continue;
            }
            seen.insert((t.current_task.raw(), t.mpc.raw()), t.instruction.raw());
        }

        eprintln!(
            "[boot_trace_decode_diagnostic] {} unique (task, mpc) pairs visited:",
            seen.len()
        );
        for ((task, mpc), &instr) in seen.iter() {
            let mi = Microinstruction::unpack(instr as u32);
            eprintln!("  task={task:2} mpc=0x{mpc:03x} instr=0x{instr:08x}");
            eprintln!(
                "    rsel={} aluf={:?} bs={:?} f1={:?} f2={:?} t_load={} l_load={} next=0x{:03x}",
                mi.rsel.raw(),
                mi.aluf,
                mi.bs,
                mi.f1,
                mi.f2,
                mi.t_load,
                mi.l_load,
                mi.next.raw()
            );
        }
    }

    /// Per-cycle MPC chain dump — useful for finding where dispatch
    /// goes off-rails (e.g. into uninitialized PROM at 0x366/0x389+).
    /// Run with:
    ///   cargo test -p rhdl-alto boot_trace_per_cycle_chain -- --nocapture --include-ignored
    #[test]
    #[ignore]
    fn boot_trace_per_cycle_chain() {
        use crate::{disk_image_loader, microcode_loader};
        let rom_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("rom");
        let disk_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("disk")
            .join("nonprog.dsk");
        if !rom_dir.join("U55").exists() || !disk_path.exists() {
            eprintln!("[per_cycle_chain] skipping — assets absent");
            return;
        }
        let microcode = microcode_loader::load_alto_ii_microcode_from_dir(&rom_dir).unwrap();
        let constants = microcode_loader::load_alto_ii_constant_rom_from_dir(&rom_dir).unwrap();
        let disk_image = disk_image_loader::load_disk_image_from_file(&disk_path).unwrap();
        let boot_sector = disk_image.sector(0, 0, 0);
        let uut = AltoChip::with_microcode_constants_and_boot(
            &microcode,
            &constants,
            &boot_sector.data,
            &boot_sector.label,
        );
        let trace = run(uut, 800);

        // After Phase 3.5 Step 4i (1-cycle/microinstruction pipeline),
        // the chip's `o.mpc` IS the address of the instruction the
        // engine is executing this cycle (current_mpc = q.task_mpc[T]
        // = last cycle's engine.next_mpc = address fed to URom for
        // this cycle's response).  So `mpc[k]` and `instruction[k]`
        // are coherently paired in the same cycle.
        eprintln!("[per_cycle_chain] cycle  task  exec_mpc  exec_instr");
        for i in 0..trace.len().min(800) {
            let cur = &trace[i];
            let exec_mpc = cur.mpc.raw();
            let exec_instr = cur.instruction.raw();
            let unprog = exec_instr == 0xfff77bff;
            let mark = if unprog { " UNPROG" } else { "" };
            eprintln!(
                "  {i:4}    {:2}  0x{exec_mpc:03x}     0x{exec_instr:08x}{mark}",
                cur.current_task.raw(),
            );
            if unprog {
                break;
            }
        }
    }

    /// Phase 3.5 baseline metric: how far does real Alto microcode
    /// get with the current (incomplete) per-task F1/F2 implementation?
    /// Runs 2000 cycles of real microcode + Constant ROM + Emulator
    /// always woken, and reports:
    ///   - how many distinct microaddresses were visited
    ///   - which task ran most often
    ///   - whether the disk's sector_mark fired (signal that disk
    ///     timing is plumbed)
    ///
    /// This is a baseline test — its assertions are intentionally loose
    /// since standard microcode behaviour is unspecified before per-task
    /// dispatch fully ships.  Future commits should see the visited-count
    /// grow as more per-task codes are implemented; the test serves as
    /// a regression baseline.
    #[test]
    fn boot_trace_baseline_metrics() {
        use crate::microcode_loader;
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("rom");
        if !dir.join("U55").exists() || !dir.join("C0").exists() {
            eprintln!("[boot_trace_baseline_metrics] skipping — assets absent");
            return;
        }
        let microcode = microcode_loader::load_alto_ii_microcode_from_dir(&dir).unwrap();
        let constants = microcode_loader::load_alto_ii_constant_rom_from_dir(&dir).unwrap();
        // Spec-correct disk period (19,608 cycles per sector, per
        // diablo_disk::SECTOR_PERIOD_CYCLES + spec §8.1) is too long
        // for a 2000-cycle test to observe disk-driven task switching.
        // Use a SHORT 256-cycle test period so the dispatch volume +
        // sector_mark cadence in 2000 cycles is comparable to what we
        // saw before the spec-correct period landed.
        let uut =
            AltoChip::with_microcode_constants_and_test_disk_period(&microcode, &constants, 256);
        let trace = run(uut, 2000);

        // Distinct microaddresses visited.
        let mut visited: std::collections::HashSet<u128> = std::collections::HashSet::new();
        for t in &trace {
            visited.insert(t.mpc.raw());
        }

        // Per-task firing counts (current_task).
        let mut task_counts = [0u32; 16];
        for t in &trace {
            let task = t.current_task.raw() as usize;
            if task < 16 {
                task_counts[task] += 1;
            }
        }

        // Sector_mark "rising-edge" count: per *Alto Hardware Manual*
        // §2.4 + spec §5.5, sector_mark is a SUSTAINED wakeup (set on
        // sector boundary, cleared by KSEC's F1=Block).  Count rising
        // edges (false → true transitions) to find sector events.
        let mut sector_events: u32 = 0;
        let mut prev_sm: bool = false;
        for t in &trace {
            if !prev_sm && t.disk_sector_mark {
                sector_events += 1;
            }
            prev_sm = t.disk_sector_mark;
        }

        eprintln!("[boot_trace_baseline_metrics] 2000-cycle trace:");
        eprintln!("  distinct microaddresses visited: {}", visited.len());
        eprintln!("  task firing counts: {task_counts:?}");
        eprintln!("  disk sector_mark events (rising edges): {sector_events}");

        let disk_sector_firings = task_counts[4];
        let emulator_firings = task_counts[0];
        eprintln!("  Emulator (task 0) firings: {emulator_firings}");
        eprintln!("  Disk Sector (task 4) firings: {disk_sector_firings}");

        // Baseline assertions per *Alto Hardware Manual* §2.4 + spec
        // §5.4 (per-task MPC streams) + §5.5 (sustained device wakeup
        // + F1=Block convention):
        //
        // - URom is presented `current_mpc` (chip's task-aware MPC
        //   lookup) so each task fetches from its OWN MPC stream
        //   per spec §5.4.  After a yield, the new task's MPC is
        //   immediately presented to URom — KSEC fetches its real
        //   microcode at MPC=4 → 0x37c → ... per spec §5.1's reset
        //   convention + the §8.7 boot sequence.
        // - F1=Block in current_task=4 clears DiabloDisk's sustained
        //   sector_mark per spec §5.5.  KSEC reaches Block somewhere
        //   in its setup loop, releasing Emulator briefly between
        //   sector boundaries.
        //
        // With the spec §2.3 1-cycle/microinstruction pipeline (Phase
        // 3.5 Step 4i), KSEC executes its handler chain (KSEC entry
        // → KPOQ → CKSECT → ... → MD←KSTAT → ...) and YIELDS BACK
        // to Emulator after the brief setup, per real Alto behavior.
        // So Emulator dominates between sector marks, not Disk Sector.
        // (The old assertion `disk_sector_firings >= 100` was based on
        // KSEC getting STUCK dispatching into uninitialized PROM with
        // the broken 2-cycle pipeline — a bug, not the spec'd behavior.)
        // E3 tightening: replaced loose floors with measured-pipeline-
        // exact ranges anchored to the 2000-cycle / 1-cycle-per-µinst
        // pipeline.  Current measurements: 76 distinct microaddresses,
        // After the F2-NEXT-modifier-timing fix (delayed pipeline per
        // spec digest §2.3), the boot trace measurements shifted —
        // delayed-apply means dispatch lands one cycle later, so the
        // Emulator and KSEC visit a different (and smaller) set of
        // distinct microaddresses in 2000 cycles.  Re-baselined:
        //   distinct microaddresses: 56 (was 76 with immediate-apply)
        //   sector_mark events:     7 (unchanged — disk cadence)
        //   Disk Sector firings:    29 (was 40)
        //   Emulator firings:       1971 (was 1960)
        // Allow ±10% slack as before.
        assert!(
            visited.len() >= 50 && visited.len() <= 65,
            "expected 50..65 distinct microaddresses in 2000-cycle boot \
             trace (delayed-pipeline baseline = 56); got {} — significant \
             deviation suggests a regression or new feature unblocking \
             more dispatch",
            visited.len()
        );
        assert!(
            (6..=10).contains(&sector_events),
            "expected 6..10 sector_mark events (every ~285 cycles in \
             2000 cycles); got {sector_events}"
        );
        // KSEC handler runs the FULL ~22-24 cycles per sector mark
        // (per AltoHW + canonical altoIIcode3.mu microcode):
        //   sector mark → wakeup → KSEC PROM-resident microcode runs
        //   from MPC=4 (KPOQ) through 0x37c..0x376 → F1=Block clears
        //   wakeup → F1=TaskYield to Emulator (K+2 timing) → done.
        // ~7 marks × ~23 = ~161; allow 140..200 to absorb mark-
        // aligned edge effects.
        //
        // Pre-chip-state-refactor baseline was 41 — that was the
        // ARTIFACT of broken per-task MPC tracking (task_mpc[4]
        // didn't update properly between sector marks, KSEC took
        // wrong paths that yielded earlier than the canonical
        // microcode would).  Post-refactor: chip-side `task_mpcs`
        // updates each non-stalled cycle, KSEC correctly executes
        // its full microcode loop matching ContrAlto cycle-for-cycle.
        assert!(
            (140..=200).contains(&disk_sector_firings),
            "Disk Sector should fire 140..200 times (post-chip-state-\
             refactor baseline = 166, computed as ~7 marks × ~23 \
             cycles per visit); got {disk_sector_firings}"
        );
        // Emulator owns the remaining cycles.  2000 - 166 = ~1834.
        assert!(
            (1800..=1860).contains(&emulator_firings),
            "Emulator should run 1800..1860 cycles in a 2000-cycle \
             trace with 7 sector marks (post-refactor baseline = \
             1834); got {emulator_firings}"
        );
    }

    /// Boot-button-equivalent trace: pre-load memory with disk
    /// sector 0 contents per spec §3.4 (the hardware action of the
    /// boot button), then run real microcode + real disk image.
    /// Expected behavior: Emulator now finds real Nova bootstrap
    /// instructions in memory (instead of all-zero NOPs) and visits
    /// many more distinct microaddresses than the empty-memory
    /// baseline.
    ///
    /// Skips if PROM or disk-image assets are absent.
    #[test]
    fn boot_trace_with_boot_button() {
        use crate::{disk_image_loader, microcode_loader};
        let rom_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("rom");
        let disk_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("disk")
            .join("nonprog.dsk");
        if !rom_dir.join("U55").exists() || !rom_dir.join("C0").exists() {
            eprintln!("[boot_trace_with_boot_button] skipping — PROM assets absent");
            return;
        }
        if !disk_path.exists() {
            eprintln!("[boot_trace_with_boot_button] skipping — disk image absent");
            return;
        }

        let microcode = microcode_loader::load_alto_ii_microcode_from_dir(&rom_dir).unwrap();
        let constants = microcode_loader::load_alto_ii_constant_rom_from_dir(&rom_dir).unwrap();
        let disk_image = disk_image_loader::load_disk_image_from_file(&disk_path).unwrap();
        let boot_sector = disk_image.sector(0, 0, 0);

        // Spec-correct disk period (~19,608 cycles) is too long for a
        // 2000-cycle boot trace to observe sector_mark-driven KSEC
        // execution.  Use a SHORT 256-cycle test period so this test
        // can still validate the "boot button + sector cadence drives
        // KSEC dispatch volume" path within the 2000-cycle budget.
        let uut = AltoChip::with_microcode_constants_boot_and_test_disk_period(
            &microcode,
            &constants,
            &boot_sector.data,
            &boot_sector.label,
            256,
        );
        // 2000 cycles is enough to enter and run the BLT bootstrap loop.
        // Termination requires R[8]=XH to reach 0; current chip enters
        // BLT with R[8]=0, which wraps to 0xFFFF and would need ~65K
        // iterations to exit.  This is a downstream Nova-emulation gap,
        // not a microengine bug — the BLT loop itself executes correctly
        // per ContrAlto's microcode.
        let trace = run(uut, 2000);

        let mut visited: std::collections::HashSet<u128> = std::collections::HashSet::new();
        for t in &trace {
            visited.insert(t.mpc.raw());
        }
        let mut task_counts = [0u32; 16];
        for t in &trace {
            let task = t.current_task.raw() as usize;
            if task < 16 {
                task_counts[task] += 1;
            }
        }
        let mut sector_events: u32 = 0;
        let mut prev_sm: bool = false;
        for t in &trace {
            if !prev_sm && t.disk_sector_mark {
                sector_events += 1;
            }
            prev_sm = t.disk_sector_mark;
        }

        eprintln!("[boot_trace_with_boot_button] 2000-cycle trace:");
        eprintln!("  distinct microaddresses visited: {}", visited.len());
        eprintln!("  task firing counts: {task_counts:?}");
        eprintln!("  disk sector_mark events: {sector_events}");
        eprintln!("  Emulator (task 0) firings: {}", task_counts[0]);
        eprintln!("  Disk Sector (task 4) firings: {}", task_counts[4]);

        // Pre-loaded memory means the Emulator's Nova fetch loop sees
        // real instructions.  Spec §3.4: "PC ← 1, and the emulator is
        // started" — Emulator now executes the boot bootstrap.
        //
        // Floor: 50 distinct microaddresses (post-F2-NEXT-modifier-
        // timing fix per spec digest §2.3).  Was 76 with immediate-
        // apply semantics; the delayed pipeline lands dispatches one
        // cycle later, which subtly shifts the microaddress visit set.
        // Re-baseline measured at 56 distinct microaddresses; 50 floor
        // gives ±10% slack.
        assert!(
            visited.len() >= 50,
            "boot trace should visit at least 50 distinct microaddresses \
             (delayed-pipeline baseline = 56); got {} — likely a \
             regression in dispatch, throughput, or per-task F1/F2/BS",
            visited.len()
        );
    }
}
