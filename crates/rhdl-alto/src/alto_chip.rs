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
    /// Sticky current-task DFF.  Per the Alto Hardware Manual §2.4,
    /// task switches happen ONLY when microcode does F1=TASK; until
    /// then the same task runs.  This DFF latches the priority-encoded
    /// winning task on engine.task_yield and otherwise holds.
    ///
    /// Replaces the previous per-cycle arbitration via task_system's
    /// last_task — task_system stays as the rhdl-rule showcase but is
    /// no longer the source of `current_task` (kept for trace
    /// observability + per-task fire counters).
    current_task: rhdl_fpga::core::dff::DFF<Bits<4>>,
    /// Per-task "has started" bitmap (16 bits, one per task).  Bit K
    /// is set the first time task K runs.  Per *Alto Hardware Manual*
    /// §2 "Initialization" + ContrAlto's `Task.cs` `Reset()`: each
    /// task K resets to MPC = K.  Until task K has run, the chip
    /// substitutes K for `task_mpc[K]` in the engine's MPC lookup,
    /// matching the real Alto's per-task reset MPC convention without
    /// requiring a custom reset value in the per-task DFF (which would
    /// diverge between Rust sim init and Verilog `initial begin`).
    task_started: rhdl_fpga::core::dff::DFF<Bits<16>>,
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
            current_task: rhdl_fpga::core::dff::DFF::new(bits::<4>(0)),
            task_started: rhdl_fpga::core::dff::DFF::new(bits::<16>(0)),
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
            current_task: rhdl_fpga::core::dff::DFF::new(bits::<4>(0)),
            task_started: rhdl_fpga::core::dff::DFF::new(bits::<16>(0)),
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
            current_task: rhdl_fpga::core::dff::DFF::new(bits::<4>(0)),
            task_started: rhdl_fpga::core::dff::DFF::new(bits::<16>(0)),
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
            current_task: rhdl_fpga::core::dff::DFF::new(bits::<4>(0)),
            task_started: rhdl_fpga::core::dff::DFF::new(bits::<16>(0)),
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

        Self {
            urom: MicrocodeRom::with_words(microcode_words),
            crom: ConstantRom::with_constants(constants),
            mem: Memory::new(mem_init),
            tasks: AltoTaskSystem::default(),
            disk_ctrl: DiskController::default(),
            disk: DiabloDisk::with_sector(boot_sector_data),
            engine: Microengine::default(),
            current_task: rhdl_fpga::core::dff::DFF::new(bits::<4>(0)),
            task_started: rhdl_fpga::core::dff::DFF::new(bits::<16>(0)),
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

    // Sticky current task (per Alto Hardware Manual §2.4): same task
    // runs every cycle until microcode does F1=TASK.  This DFF latches
    // the priority-encoded winning task on engine.task_yield; otherwise
    // holds.  task_system's `last_task` is now used only for trace
    // observability + per-task fire counters.
    let current_task: Bits<4> = q.current_task;
    // Look up that task's MPC.  Per *Alto Hardware Manual* §2
    // "Initialization" + ContrAlto's `Task.cs` Reset(): each task K
    // resets to MPC = K.  q.tasks.task_mpc DFFs default to all zeros
    // (a constraint of the synthesizable DFF reset value matching
    // the Verilog `initial begin` for the per-task array), so we
    // substitute K for task_mpc[K] until task K has run at least once.
    // The task_started bitmap tracks which tasks have run.
    let task_bit: Bits<16> = bits::<16>(1) << current_task.resize::<5>();
    let task_has_started: bool = (q.task_started & task_bit) != bits::<16>(0);
    let current_mpc: Bits<10> = if task_has_started {
        q.tasks.task_mpc[current_task]
    } else {
        current_task.resize::<10>()
    };

    // Microcode RAM has 1-cycle BRAM latency.  q.urom.instruction is
    // the instruction at the address presented last cycle.
    let instr_this_cycle: Bits<32> = q.urom.instruction;

    // Decode RSEL[31:27] and BS[22:20] from the instruction; constant
    // ROM index = (RSEL << 3) | BS = 8 bits.
    let rsel: Bits<5> = ((instr_this_cycle >> 27) & bits::<32>(0x1F)).resize();
    let bs:   Bits<3> = ((instr_this_cycle >> 20) & bits::<32>(0x07)).resize();
    let const_idx: Bits<8> = (rsel.resize::<8>() << 3) | bs.resize::<8>();
    d.crom = ConstantIn { index: const_idx };
    let const_value: Bits<16> = q.crom.value;

    // Memory bus.
    let mem_data_for_engine: Bits<16> = q.mem.read_data;

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
    };
    d.mem = MemIn {
        address: q.engine.mem_address,
        write_data: q.engine.mem_write_data,
        write_en: q.engine.mem_write_en,
    };

    // Engine outputs next_mpc this cycle.
    let next_mpc: Bits<10> = q.engine.next_mpc;
    // URom fetch address per *Alto Hardware Manual* §2.4 / spec §5.4.
    // Each task has its own MPC stream — when the chip switches
    // current_task, the URom must immediately start fetching from
    // the new task's saved MPC (the spec table in §5.4 shows J — the
    // first new-task instruction — being fetched the cycle after
    // TASK fires, with the new task's MPC).
    //
    // We can't read `engine.next_mpc` combinationally from the chip
    // (it's Q-register-delayed), so we DON'T present `q.engine.next_mpc`
    // to URom directly — that would always follow the OLD task's
    // stream after a yield, since q.engine.next_mpc was computed
    // before current_task changed.  Instead we present `current_mpc`
    // (the chip's task-aware MPC lookup), which IS the new task's
    // MPC the cycle current_task switches.  task_mpc[current_task]
    // is updated each cycle by the arbiter rule from
    // `next_mpc_per_task[current_task] = engine.next_mpc`, so within
    // a single task's stream the URom advances one instruction per
    // (2-cycle pipeline) just as before.  On task switch, current_mpc
    // immediately reflects the new task — matching real Alto's
    // 1-delay-slot pipeline.
    d.urom = UromIn { mpc: current_mpc.resize() };

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
    disk_in.transfer_request = q.disk_ctrl.transfer_request
        || q.engine.disk_strobe;
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
    if disk_sector_mark { effective_wakeups = effective_wakeups | bits::<16>(0x0010); }
    if disk_word_strobe { effective_wakeups = effective_wakeups | bits::<16>(0x4000); }

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
    let winning_task: Bits<4> =
             if (effective_wakeups & bits::<16>(0x4000)) != bits::<16>(0) { bits::<4>(14) }
        else if (effective_wakeups & bits::<16>(0x2000)) != bits::<16>(0) { bits::<4>(13) }
        else if (effective_wakeups & bits::<16>(0x1000)) != bits::<16>(0) { bits::<4>(12) }
        else if (effective_wakeups & bits::<16>(0x0800)) != bits::<16>(0) { bits::<4>(11) }
        else if (effective_wakeups & bits::<16>(0x0400)) != bits::<16>(0) { bits::<4>(10) }
        else if (effective_wakeups & bits::<16>(0x0200)) != bits::<16>(0) { bits::<4>(9)  }
        else if (effective_wakeups & bits::<16>(0x0100)) != bits::<16>(0) { bits::<4>(8)  }
        else if (effective_wakeups & bits::<16>(0x0080)) != bits::<16>(0) { bits::<4>(7)  }
        else if (effective_wakeups & bits::<16>(0x0010)) != bits::<16>(0) { bits::<4>(4)  }
        else if (effective_wakeups & bits::<16>(0x0001)) != bits::<16>(0) { bits::<4>(0)  }
        else { current_task };
    // Latch winning_task into current_task only when engine.task_yield
    // (F1=TASK).  Otherwise hold (sticky).
    d.current_task = if q.engine.task_yield { winning_task } else { current_task };
    // Mark current_task as "started" — the next time it's selected by
    // arbitration, the chip will read its accumulated task_mpc rather
    // than the per-task reset MPC = K.
    d.task_started = q.task_started | task_bit;

    // Outputs
    o.mpc              = current_mpc;
    o.current_task     = current_task;
    o.next_mpc         = next_mpc;
    o.t                = q.engine.t;
    o.l                = q.engine.l;
    o.bus              = q.engine.bus;
    o.alu_result       = q.engine.alu_result;
    o.instruction      = instr_this_cycle;
    o.disk_sector_mark    = disk_sector_mark;
    o.disk_word_strobe    = disk_word_strobe;
    o.wakeups             = effective_wakeups;
    o.disk_ctrl_read_data = q.disk_ctrl.read_data;
    o.disk_ctrl_write_en  = q.engine.disk_ctrl_write_en;
    o.ir                  = q.engine.ir;
    o.disk_sector_count   = q.tasks.disk_sector_count;
    o.disk_word_count     = q.tasks.disk_word_count;
    o.mem_write_observed_addr = q.engine.mem_address;
    o.mem_write_observed_data = q.engine.mem_write_data;
    o.mem_write_observed_en   = q.engine.mem_write_en;

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::isa::*;

    /// Pack a simple microinstruction.
    fn ui(rsel: u8, aluf: AluFunction, bs: BusSource, t_load: bool, l_load: bool, next: u16) -> u32 {
        Microinstruction {
            rsel: bits::<5>(rsel as u128),
            aluf,
            bs,
            f1: F1Function::Nop,
            f2: F2Function::Nop,
            t_load,
            l_load,
            next: bits::<10>(next as u128),
        }.pack()
    }

    /// Boot input: wake Task 0 (Emulator) only, every cycle.
    fn boot_in() -> ChipIn {
        ChipIn { wakeups: bits::<16>(0x0001) }
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
        assert_eq!(trace.last().unwrap().mpc.raw(), 0, "MPC stays at 0 in the loop");
    }

    #[test]
    fn boot_branches_to_addr_3_via_bus_eq_zero() {
        // Microcode at addr 0: F2=BusEqZero with NEXT=2, ALUF=BusPlusOne, L_LOAD=1.
        //   bus = R[0] = 0 → F2 sets bit 0 of NEXT → next = 3.
        // Microcode at addr 3: NOP loop (NEXT=3).
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        let mi0 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::BusPlusOne,
            bs: BusSource::ReadR,
            f1: F1Function::Nop,
            f2: F2Function::BusEqZero,
            t_load: false, l_load: true,
            next: bits::<10>(2),
        };
        microcode[0] = mi0.pack();
        microcode[3] = ui(0, AluFunction::Bus, BusSource::None, false, false, 3);

        let uut = AltoChip::with_microcode(&microcode);
        let trace = run(uut, 8);

        // MPC should reach 3 and stay there.
        // Allow a few cycles of pipeline settling.
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
            bs: BusSource::ReadR,  // overridden by F1=Constant
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
        assert_eq!(final_l, 0x1234,
            "L should latch the constant ROM value via F1=Constant");
    }

    #[test]
    fn f1_constant_with_different_index() {
        // RSEL=0b00001 (1), BS=0b010 (2) → index = (1 << 3) | 2 = 10.
        let mi0 = Microinstruction {
            rsel: bits::<5>(1),
            aluf: AluFunction::Bus,
            bs: BusSource::None,  // BS=2; overridden by F1=Constant for BUS
            f1: F1Function::Constant,
            f2: F2Function::Nop,
            t_load: false, l_load: true,
            next: bits::<10>(0),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi0.pack();
        let mut constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        constants[10] = 0xCAFE;
        let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
        let trace = run(uut, 8);
        assert_eq!(trace.last().unwrap().l.raw(), 0xCAFE,
            "L should latch the constant at index (RSEL<<3)|BS = 10");
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
            rsel: bits::<5>(0), aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::LoadMar, f2: F2Function::Constant,
            t_load: false, l_load: false, next: bits::<10>(1),
        };
        // addr 1: filler — wait one cycle for BRAM read to land.
        let mi1 = Microinstruction {
            rsel: bits::<5>(0), aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::Nop, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(2),
        };
        // addr 2: BS=MemoryData + L_LOAD → L ← memory[MAR] (= 0x0080's value)
        //         loop here.
        let mi2 = Microinstruction {
            rsel: bits::<5>(0), aluf: AluFunction::Bus, bs: BusSource::MemoryData,
            f1: F1Function::Nop, f2: F2Function::Nop,
            t_load: false, l_load: true, next: bits::<10>(2),
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

        let uut = AltoChip::with_microcode_constants_and_memory(
            &microcode, &constants, memory_initial,
        );
        let trace = run(uut, 16);

        // Eventually L should hold 0xC0DE (the memory value at 0x0080).
        let final_l = trace.last().unwrap().l.raw();
        assert_eq!(final_l, 0xC0DE,
            "L should latch the memory value at 0x0080 after MAR + MD→ sequence");
    }

    /// Memory write end-to-end through AltoChip: use F1=Constant to
    /// load address, F2=LoadMar.  Then F1=Constant to load value,
    /// F2=WriteMd to write to memory[MAR].  Finally read back via
    /// BS=MemoryData and verify L holds the written value.
    #[test]
    fn memory_write_then_read_round_trip() {
        // addr 0: F1=Constant idx 0 = 0x0100 (target addr) + F2=LoadMar
        let mi0 = Microinstruction {
            rsel: bits::<5>(0), aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::LoadMar, f2: F2Function::Constant,
            t_load: false, l_load: false, next: bits::<10>(1),
        };
        // addr 1: F1=Constant idx 1 = 0xBEEF + F2=StoreMd → memory[0x0100] ← 0xBEEF
        //         (RSEL=0, BS=1 → constant index = 1).  F1=Constant sets
        //         BUS from constant ROM; F2=StoreMd writes BUS to memory.
        let mi1 = Microinstruction {
            rsel: bits::<5>(0), aluf: AluFunction::Bus, bs: BusSource::LoadR, // BS=1
            f1: F1Function::Constant, f2: F2Function::StoreMd,
            t_load: false, l_load: false, next: bits::<10>(2),
        };
        // addr 2: filler (BRAM write commit + read latency)
        let mi2 = Microinstruction {
            rsel: bits::<5>(0), aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::Nop, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(3),
        };
        // addr 3: another filler
        let mi3 = Microinstruction {
            rsel: bits::<5>(0), aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::Nop, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(4),
        };
        // addr 4: BS=MemoryData + L_LOAD → L ← memory[MAR] (= 0xBEEF).  Loop.
        let mi4 = Microinstruction {
            rsel: bits::<5>(0), aluf: AluFunction::Bus, bs: BusSource::MemoryData,
            f1: F1Function::Nop, f2: F2Function::Nop,
            t_load: false, l_load: true, next: bits::<10>(4),
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
        assert_eq!(final_l, 0xBEEF,
            "after write-then-read sequence, L should hold the written 0xBEEF");
    }

    /// Multi-task arbitration through the chip: wake Task 0 (Emulator)
    /// AND Task 4 (Disk Sector) every cycle.  Task 4 has higher priority
    /// (lower priority-number in the rule), so it fires.  Verify
    /// out.current_task reflects the firing task.
    #[test]
    fn multi_task_arbitration_picks_higher_priority() {
        // Microcode at addr 0: F1=TaskYield → engine asserts task_yield;
        // chip latches priority-encoder winner into current_task.
        // Per Alto Hardware Manual §2.4, task switches happen ONLY on
        // F1=TASK; the test microcode must therefore issue it explicitly
        // for arbitration to occur.
        let mi0 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::TaskYield, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(0),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi0.pack();
        let constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
        // Wake both Task 0 (bit 0) and Task 4 (bit 4 = 0x0010).
        let inputs: Vec<ChipIn> = (0..16).map(|_| ChipIn {
            wakeups: bits::<16>(0x0011),
        }).collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let trace: Vec<ChipOut> = uut.run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect();
        // After the initial settling cycles, current_task should be 4
        // (Disk Sector wins over Emulator on shared wakeup).
        let final_task = trace.last().unwrap().current_task.raw();
        assert_eq!(final_task, 4,
            "with both task 0 and task 4 woken, task 4 (Disk Sector) wins");
    }

    /// Per-task gating: under Disk Sector (Task 4), F1=WriteKadr
    /// asserts disk_ctrl_write_en; under Emulator (Task 0), the
    /// same instruction is a no-op.
    #[test]
    fn f1_write_kadr_only_active_under_disk_sector_task() {
        // Microcode at addr 0: F1=TaskYield (so the chip arbitrates and
        // switches to the highest-priority woken task; without this,
        // current_task stays at the DFF default of 0 forever per Alto
        // Hardware Manual §2.4 sticky-task semantics).
        // Microcode at addr 1: F1=WriteKadr, RSEL=0, BS=ReadR.
        // (BUS = R[0] = 0; we care only about the write_en signal.)
        let mi0 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::TaskYield, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(1),
        };
        let mi1 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::WriteKadr, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(1),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi0.pack();
        microcode[1] = mi1.pack();
        let constants = [0u16; crate::constant_rom::NUM_CONSTANTS];

        // ---- Test A: Disk Sector task (4) firing → write_en asserted
        let uut_a = AltoChip::with_microcode_and_constants(&microcode, &constants);
        let inputs_a: Vec<ChipIn> = (0..16).map(|_| ChipIn {
            wakeups: bits::<16>(0x0010),  // wake Task 4 only
        }).collect();
        let stream_a = inputs_a.into_iter().with_reset(2).clock_pos_edge(100);
        let trace_a: Vec<ChipOut> = uut_a.run(stream_a)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect();
        let write_active_a = trace_a.iter().any(|t| t.disk_ctrl_write_en);
        assert!(write_active_a,
            "Under Disk Sector task, F1=WriteKadr should assert write_en");

        // ---- Test B: Emulator task (0) firing → write_en NEVER asserted
        let uut_b = AltoChip::with_microcode_and_constants(&microcode, &constants);
        let inputs_b: Vec<ChipIn> = (0..16).map(|_| ChipIn {
            wakeups: bits::<16>(0x0001),  // wake Task 0 only
        }).collect();
        let stream_b = inputs_b.into_iter().with_reset(2).clock_pos_edge(100);
        let trace_b: Vec<ChipOut> = uut_b.run(stream_b)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect();
        let write_active_b = trace_b.iter().any(|t| t.disk_ctrl_write_en);
        assert!(!write_active_b,
            "Under Emulator task, F1=WriteKadr is a no-op (gated)");
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
        // Shared microcode for both Disk Sector (Task 4) and Disk Word
        // (Task 14).  Each task's MPC advances independently; per-task
        // gating makes most instructions no-ops for the wrong task.
        //
        // Per Alto Hardware Manual §2.4, task switches happen ONLY on
        // F1=TaskYield.  The chip's current_task DFF defaults to 0
        // (Emulator); we therefore lead with a TaskYield instruction so
        // the priority arbiter switches us to Task 4 (Disk Sector).
        //
        //   addr 0: F1=TaskYield → arbitrate, switch to highest-prio
        //           woken task (Task 4 with wakeups=0x0010).
        //   addr 1: R[2] ← 0x0200 (KCWA target).  Bus from constant ROM
        //           via F1=Constant + BS=LoadR ⇒ R[RSEL=2] ← const[17].
        //   addr 2: R[1] ← 0x8000 (KCOM "start" bit).  Same shape:
        //           const[9] = 0x8000, RSEL=1, BS=LoadR.
        //   addr 3: KCWA ← R[2] = 0x0200.  F1=WriteKcwa (Task 4 only).
        //   addr 4: KCOM ← R[1] = 0x8000.  F1=WriteKcomm (Task 4 only).
        //           Arms the transfer.
        //   addr 5: F1=TaskYield → allow Disk Word (Task 14) to win
        //           arbitration when word_strobe pulses; falls through
        //           to addr 6.
        //   addr 6: NOP loop for Task 4; F2=DiskWordTransfer for Task 14
        //           — atomic DMA per cycle while word_strobe.
        let mi0 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::TaskYield, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(1),
        };
        // Per spec §2.7 + ContrAlto: R loads from Shifter.Output (L
        // after F1 shift), NOT from ALU result.  To load a constant
        // into R via F1=Constant + BS=LoadR, we must also set
        // L_LOAD=true so L latches the constant; then R ← L.
        let mi1 = Microinstruction {
            rsel: bits::<5>(2),
            aluf: AluFunction::Bus, bs: BusSource::LoadR,
            f1: F1Function::Constant, f2: F2Function::Nop,
            t_load: false, l_load: true, next: bits::<10>(2),
        };
        let mi2 = Microinstruction {
            rsel: bits::<5>(1),
            aluf: AluFunction::Bus, bs: BusSource::LoadR,
            f1: F1Function::Constant, f2: F2Function::Nop,
            t_load: false, l_load: true, next: bits::<10>(3),
        };
        let mi3 = Microinstruction {
            rsel: bits::<5>(2),
            aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::WriteKcwa, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(4),
        };
        let mi4 = Microinstruction {
            rsel: bits::<5>(1),
            aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::WriteKcomm, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(5),
        };
        let mi5 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::TaskYield, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(6),
        };
        let mi6 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::TaskYield, f2: F2Function::DiskWordTransfer,
            t_load: false, l_load: false, next: bits::<10>(6),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi0.pack();
        microcode[1] = mi1.pack();
        microcode[2] = mi2.pack();
        microcode[3] = mi3.pack();
        microcode[4] = mi4.pack();
        microcode[5] = mi5.pack();
        microcode[6] = mi6.pack();

        // ---- Constant ROM ---------------------------------------------
        // const[17] (= (RSEL=2 << 3) | BS=LoadR) → 0x0200 (KCWA target).
        // const[9]  (= (RSEL=1 << 3) | BS=LoadR) → 0x8000 (KCOM arm bit).
        let mut constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        constants[17] = 0x0200;
        constants[9]  = 0x8000;

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
            current_task: rhdl_fpga::core::dff::DFF::new(bits::<4>(0)),
            task_started: rhdl_fpga::core::dff::DFF::new(bits::<16>(0)),
        };

        // Wakeup pattern: Task 4 (Disk Sector) always woken so its
        // microcode runs.  Disk Word (Task 14) wake comes from the
        // disk's word_strobe automatically once transfer arms.
        let inputs: Vec<ChipIn> = (0..400).map(|_| ChipIn {
            wakeups: bits::<16>(0x0010),
        }).collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let trace: Vec<ChipOut> = uut.run(stream)
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

        assert!(final_disk_word_count >= 256,
            "Disk Word task should fire at least 256 times (one per DMA); got {final_disk_word_count}");
        // It shouldn't fire dramatically more, either — that would mean
        // word_strobe didn't stop, suggesting transfer_remaining never
        // hit 0 (a re-arm bug).
        assert!(final_disk_word_count <= 280,
            "Disk Word should stop firing after transfer ends; got {final_disk_word_count} (re-arm bug?)");

        // Disk Sector should have fired its setup pass (4 cycles)
        // then sat at addr 4 NOP loop.  Each cycle it doesn't fire
        // (when Task 14 wins arbitration), no count increment.  After
        // DMA ends, Disk Sector starts firing again.  Expected:
        // ~5 firings during setup + ~140 firings post-DMA = ~145.
        let final_disk_sector_count = trace.last().unwrap().disk_sector_count.raw();
        eprintln!("[end_to_end_256_word_dma] disk_sector_count = {final_disk_sector_count}");
        assert!(final_disk_sector_count >= 4,
            "Disk Sector should fire at least 4 times (setup); got {final_disk_sector_count}");

        // ---- Verify memory contents via observed-write trace --------
        // Reconstruct the engine's memory-write trace from ChipOut and
        // verify the right WORDS landed at the right addresses.
        // Expected: 256 writes of (addr, data) = (0x200+i, 0xA000+i)
        // for i in 0..256.
        let mut writes: Vec<(u128, u128)> = Vec::new();
        for t in &trace {
            if t.mem_write_observed_en {
                writes.push((t.mem_write_observed_addr.raw(),
                             t.mem_write_observed_data.raw()));
            }
        }
        eprintln!("[end_to_end_256_word_dma] observed {} memory writes", writes.len());
        // 256 DMAs expected; pipeline-delay can produce one extra
        // firing as the wakeup chain settles.  Accept 256 or 257.
        assert!(writes.len() == 256 || writes.len() == 257,
            "Expected 256 or 257 memory writes (256 DMAs + maybe 1 transition); got {}",
            writes.len());
        // Verify the first 256 writes: addr 0x200..0x300, data 0xA000..0xA0FF.
        for (i, &(addr, data)) in writes.iter().take(256).enumerate() {
            let expected_addr = 0x200u128 + (i as u128);
            let expected_data = 0xA000u128 + (i as u128);
            assert_eq!(addr, expected_addr,
                "Write {i}: addr = {addr:#06x}, expected {expected_addr:#06x}");
            assert_eq!(data, expected_data,
                "Write {i}: data = {data:#06x}, expected {expected_data:#06x}");
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
        // addr 0: F1=TaskYield → switch to Disk Sector (the only woken
        //         task in this test) per Alto Hardware Manual §2.4.
        let mi0 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs:   BusSource::ReadR,
            f1:   F1Function::TaskYield,
            f2:   F2Function::Nop,
            t_load: false, l_load: false,
            next: bits::<10>(1),
        };
        // addr 1: F1=Constant idx 9 = 0x8000, BS=LoadR, RSEL=1, ALUF=Bus
        //         → R[1] ← 0x8000.  NEXT=2.
        // L_LOAD=true so L gets the constant; R loads from L (Shifter
        // Output) per spec §2.7 + ContrAlto's Task.cs.
        let mi1 = Microinstruction {
            rsel: bits::<5>(1),
            aluf: AluFunction::Bus,
            bs:   BusSource::LoadR,
            f1:   F1Function::Constant,
            f2:   F2Function::Nop,
            t_load: false, l_load: true,
            next: bits::<10>(2),
        };
        // addr 2: BS=ReadR (RSEL=1 → BUS = R[1] = 0x8000), F1=WriteKcomm
        //         → KCOM ← 0x8000.
        let mi2 = Microinstruction {
            rsel: bits::<5>(1),
            aluf: AluFunction::Bus,
            bs:   BusSource::ReadR,
            f1:   F1Function::WriteKcomm,
            f2:   F2Function::Nop,
            t_load: false, l_load: false,
            next: bits::<10>(3),
        };
        // addr 3: F1=TaskYield → release Disk Sector so Disk Word
        //         (Task 14) can win arbitration when word_strobe pulses.
        let mi3 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus,
            bs:   BusSource::ReadR,
            f1:   F1Function::TaskYield,
            f2:   F2Function::Nop,
            t_load: false, l_load: false,
            next: bits::<10>(3),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi0.pack();
        microcode[1] = mi1.pack();
        microcode[2] = mi2.pack();
        microcode[3] = mi3.pack();
        let mut constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        // F1=Constant in mi0 has RSEL=1, BS=LoadR(=1), so the constant
        // index = (1 << 3) | 1 = 9.  Put 0x8000 there.
        constants[9] = 0x8000;

        let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
        // Wake Disk Sector (task 4) only.
        let inputs: Vec<ChipIn> = (0..40).map(|_| ChipIn {
            wakeups: bits::<16>(0x0010),
        }).collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let trace: Vec<ChipOut> = uut.run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect();

        // Within 40 cycles, the chain should have completed:
        // KCOM written → transfer_request → disk armed → word_strobe fires.
        let any_word_strobe = trace.iter().any(|t| t.disk_word_strobe);
        assert!(any_word_strobe,
            "word_strobe should fire after Disk Sector microcode writes KCOM = 0x8000");

        // And: the Disk Word task (14) should have fired.
        let any_task_14 = trace.iter().any(|t| t.current_task.raw() == 14);
        assert!(any_task_14,
            "Disk Word task (14) should fire after disk arms the transfer");
    }

    /// IDispatch under Emulator: stage IR=0xCAFE then run F2=IDispatch
    /// with NEXT=0x100.  Per spec §6.6 IDISP PROM:
    ///   IR=0xCAFE → ir_1_2 = (0xCAFE >> 13) & 3 = 2 → dispatch_value = 5.
    /// So next_mpc = 0x100 | 5 = 0x105.
    #[test]
    fn f2_idispatch_routes_via_ir_low_byte() {
        // addr 0: F1=LoadMar + F2=Constant idx=0=0x0100 → MAR ← 0x0100.
        let mi0 = Microinstruction {
            rsel: bits::<5>(0), aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::LoadMar, f2: F2Function::Constant,
            t_load: false, l_load: false, next: bits::<10>(1),
        };
        // addr 1: filler, BRAM read settle.
        let mi1 = Microinstruction {
            rsel: bits::<5>(0), aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::Nop, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(2),
        };
        // addr 2: F2=LoadIr → IR ← MD = 0xCAFE.
        let mi2 = Microinstruction {
            rsel: bits::<5>(0), aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::Nop, f2: F2Function::LoadIr,
            t_load: false, l_load: false, next: bits::<10>(3),
        };
        // addr 3: F2=IDispatch with NEXT=0x100.  IR=0xCAFE.
        // Per spec §6.6 IDISP PROM: IR[1-2] = (0xCAFE >> 13) & 3 =
        // (0x6) & 3 = 2 → dispatch_value = 5.
        // next_mpc = 0x100 | 5 = 0x105.  Handler at 0x105.
        let mi3 = Microinstruction {
            rsel: bits::<5>(0), aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::Nop, f2: F2Function::IDispatch,
            t_load: false, l_load: false, next: bits::<10>(0x100),
        };
        // addr 0x105: handler — BUS+1 → L = 1, loop.
        let mi_target = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::BusPlusOne,
            bs: BusSource::ReadR,
            f1: F1Function::Nop, f2: F2Function::Nop,
            t_load: false, l_load: true,
            next: bits::<10>(0x105),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi0.pack();
        microcode[1] = mi1.pack();
        microcode[2] = mi2.pack();
        microcode[3] = mi3.pack();
        microcode[0x105] = mi_target.pack();

        let mut constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        constants[0] = 0x0100;  // MAR target
        let memory_initial = vec![(bits::<16>(0x0100), bits::<16>(0xCAFE))];

        let uut = AltoChip::with_microcode_constants_and_memory(
            &microcode, &constants, memory_initial,
        );
        let trace = run(uut, 16);

        // L should latch 1 (BUS+1 with BUS=0) at the handler.
        let final_l = trace.last().unwrap().l.raw();
        assert_eq!(final_l, 1,
            "L should latch 1 from the addr-0x105 BUS+1 ALU op, proving \
             spec §6.6 IDISP PROM routed MPC to 0x105 = 0x100 | 5 \
             (IR[1-2]=2 → dispatch=5)");
        let final_ir = trace.last().unwrap().ir.raw();
        assert_eq!(final_ir, 0xCAFE,
            "IR should hold 0xCAFE from the LoadIr step");
    }

    /// Per-task IR load: under Emulator (task 0), F2=LoadIr loads
    /// IR ← MD (memory data).  Stage memory[0x100] = 0xCAFE, do
    /// MAR ← 0x100, wait, then F2=LoadIr; verify IR == 0xCAFE.
    #[test]
    fn f2_load_ir_only_active_under_emulator_task() {
        // addr 0: F1=Constant idx 0 = 0x0100, F2=LoadMar → MAR ← 0x0100
        let mi0 = Microinstruction {
            rsel: bits::<5>(0), aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::LoadMar, f2: F2Function::Constant,
            t_load: false, l_load: false, next: bits::<10>(1),
        };
        // addr 1: filler, give BRAM read time to land
        let mi1 = Microinstruction {
            rsel: bits::<5>(0), aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::Nop, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(2),
        };
        // addr 2: F2=LoadIr  → IR ← MD (which is memory[MAR] = 0xCAFE)
        //         loop here.
        let mi2 = Microinstruction {
            rsel: bits::<5>(0), aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::Nop, f2: F2Function::LoadIr,
            t_load: false, l_load: false, next: bits::<10>(2),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi0.pack();
        microcode[1] = mi1.pack();
        microcode[2] = mi2.pack();
        let mut constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        constants[0] = 0x0100;
        let memory_initial = vec![(bits::<16>(0x0100), bits::<16>(0xCAFE))];

        let uut = AltoChip::with_microcode_constants_and_memory(
            &microcode, &constants, memory_initial,
        );
        let trace = run(uut, 16);  // Task 0 (Emulator) always woken via boot_in()
        let final_ir = trace.last().unwrap().ir.raw();
        assert_eq!(final_ir, 0xCAFE,
            "IR should latch the memory value at 0x0100 via LoadIr in Emulator task");
    }

    /// End-to-end disk → wakeup → arbiter → engine path: the disk
    /// widget's sector_mark fires every 256 cycles; with a long-enough
    /// run, the Disk Sector task (Task 4) fires at least once.
    #[test]
    fn disk_sector_mark_fires_disk_sector_task() {
        // Microcode at addr 0: F1=TaskYield loop.  Per Alto Hardware
        // Manual §2.4 sticky-task semantics, task switches happen ONLY
        // on F1=TaskYield; the Emulator's idle loop yields every cycle
        // so the priority arbiter can hand control to Task 4 the moment
        // sector_mark sets wakeup bit 4.
        let mi0 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::TaskYield, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(0),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi0.pack();
        let constants = [0u16; crate::constant_rom::NUM_CONSTANTS];
        let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
        // Wake Task 0 (Emulator) only via input; disk's sector_mark
        // should add wakeup bit 4 every 256 cycles.
        let inputs: Vec<ChipIn> = (0..512).map(|_| ChipIn {
            wakeups: bits::<16>(0x0001),
        }).collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let trace: Vec<ChipOut> = uut.run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect();
        // Verify the wakeup vector saw the disk's sector_mark at some
        // point (bit 4 set in the effective wakeup vector).
        let any_disk_wakeup = trace.iter().any(|t| (t.wakeups.raw() & 0x0010) != 0);
        assert!(any_disk_wakeup,
            "disk sector_mark should have driven wakeup bit 4 at least once");
        // Verify the Disk Sector task (Task 4) fired at least once
        // (current_task = 4 in some cycle).
        let any_task_4 = trace.iter().any(|t| t.current_task.raw() == 4);
        assert!(any_task_4,
            "Disk Sector task should have fired in response to the disk wakeup");
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
        // mi0: F1=TaskYield (switch to Disk Sector, the only woken task).
        let mi0 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::TaskYield, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(1),
        };
        // mi1: F1=STROBE (Code9, per spec §8.5).  Arms transfer.
        // Other fields are NOP-ish; the chip latches engine.disk_strobe
        // (q-registered, 1-cycle lag) and ORs into disk.transfer_request.
        let mi1 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::Code9, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(2),
        };
        // mi2: F1=TaskYield loop (release Disk Sector so Disk Word
        // can win arbitration when word_strobe fires).
        let mi2 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::TaskYield, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(2),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi0.pack();
        microcode[1] = mi1.pack();
        microcode[2] = mi2.pack();
        let constants = [0u16; crate::constant_rom::NUM_CONSTANTS];

        let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
        // Wake Disk Sector (task 4) only.
        let inputs: Vec<ChipIn> = (0..50).map(|_| ChipIn {
            wakeups: bits::<16>(0x0010),
        }).collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let trace: Vec<ChipOut> = uut.run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect();

        // After the STROBE fires, transfer arms → word_strobe asserts.
        let any_word_strobe = trace.iter().any(|t| t.disk_word_strobe);
        assert!(any_word_strobe,
            "F1=STROBE in Disk Sector should arm the transfer; \
             word_strobe should assert");
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
            .join("assets").join("rom");
        if !dir.join("U55").exists() || !dir.join("C0").exists() {
            eprintln!("[boot_with_real_microcode_and_constants_does_not_crash] skipping — assets absent");
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
        assert!(any_nonzero, "should fetch at least one non-zero microinstruction from real microcode");
        // It should also have visited multiple distinct microaddresses
        // (the boot path doesn't immediately stick at one address).
        let mut visited: std::collections::HashSet<u128> = std::collections::HashSet::new();
        for t in &trace {
            visited.insert(t.mpc.raw());
        }
        assert!(visited.len() >= 2,
            "boot trace should visit at least 2 distinct microaddresses; visited {:?}",
            visited);
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
            .join("assets").join("rom");
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

        eprintln!("[boot_trace_decode_diagnostic] {} unique (task, mpc) pairs visited:", seen.len());
        for ((task, mpc), &instr) in seen.iter() {
            let mi = Microinstruction::unpack(instr as u32);
            eprintln!("  task={task:2} mpc=0x{mpc:03x} instr=0x{instr:08x}");
            eprintln!("    rsel={} aluf={:?} bs={:?} f1={:?} f2={:?} t_load={} l_load={} next=0x{:03x}",
                mi.rsel.raw(), mi.aluf, mi.bs, mi.f1, mi.f2,
                mi.t_load, mi.l_load, mi.next.raw());
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
            .join("assets").join("rom");
        if !dir.join("U55").exists() || !dir.join("C0").exists() {
            eprintln!("[boot_trace_baseline_metrics] skipping — assets absent");
            return;
        }
        let microcode = microcode_loader::load_alto_ii_microcode_from_dir(&dir).unwrap();
        let constants = microcode_loader::load_alto_ii_constant_rom_from_dir(&dir).unwrap();
        let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
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
            if task < 16 { task_counts[task] += 1; }
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
        // Observable: distinct microaddresses ≥ 30 (KSEC's real
        // microcode is running), sector_mark events ≥ 2 (KSEC hits
        // Block, which clears sector_mark; next sector tick fires
        // another mark), Disk Sector dominates total cycles (KSEC's
        // setup loop is much longer than Emulator's tight boot loop).
        assert!(visited.len() >= 30,
            "real KSEC microcode should visit ≥30 distinct microaddresses \
             (was ≤8 before per-task MPC stream alignment); got {}", visited.len());
        assert!(sector_events >= 2,
            "F1=Block in KSEC clears sustained sector_mark; subsequent \
             sector tick re-fires it.  Expected ≥2 events in 2000 cycles, \
             saw {sector_events}");
        assert!(disk_sector_firings >= 100,
            "once sector_mark fires Disk Sector dominates; got {disk_sector_firings}");
        assert!(emulator_firings > 0,
            "Emulator should fire at least once during initial boot before \
             the first sector_mark; saw {emulator_firings}");
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
            .join("assets").join("rom");
        let disk_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets").join("disk").join("nonprog.dsk");
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

        let uut = AltoChip::with_microcode_constants_and_boot(
            &microcode,
            &constants,
            &boot_sector.data,
            &boot_sector.label,
        );
        let trace = run(uut, 2000);

        let mut visited: std::collections::HashSet<u128> =
            std::collections::HashSet::new();
        for t in &trace {
            visited.insert(t.mpc.raw());
        }
        let mut task_counts = [0u32; 16];
        for t in &trace {
            let task = t.current_task.raw() as usize;
            if task < 16 { task_counts[task] += 1; }
        }
        let mut sector_events: u32 = 0;
        let mut prev_sm: bool = false;
        for t in &trace {
            if !prev_sm && t.disk_sector_mark { sector_events += 1; }
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
        // We don't predict exact microaddress count (depends on what
        // Nova instructions the bootstrap runs and how many distinct
        // microcode handlers they reach), but we expect MORE than
        // the empty-memory baseline (35 addresses).
        assert!(visited.len() >= 35,
            "boot-button-loaded memory should drive Emulator through \
             at least as many addresses as empty memory; got {}",
            visited.len());
    }
}
