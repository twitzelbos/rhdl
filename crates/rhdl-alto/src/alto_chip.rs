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

    // Determine which task is running this cycle.  The arbiter's
    // `last_task` is registered (1 cycle behind), which matches the
    // Alto's MIF/MIE pipeline: arbitration happens at cycle T, the
    // chosen task's microinstruction executes at cycle T+1.
    let current_task: Bits<4> = q.tasks.last_task;
    // Look up that task's MPC.
    let current_mpc: Bits<10> = q.tasks.task_mpc[current_task];

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
    d.engine = MicroIn {
        mpc: current_mpc,
        instr: instr_this_cycle,
        constant_value: const_value,
        mem_read_data: mem_data_for_engine,
        current_task,
    };
    d.mem = MemIn {
        address: q.engine.mem_address,
        write_data: q.engine.mem_write_data,
        write_en: q.engine.mem_write_en,
    };

    // Engine outputs next_mpc this cycle.
    let next_mpc: Bits<10> = q.engine.next_mpc;
    d.urom = UromIn { mpc: next_mpc.resize() };

    // Disk: not yet driven by microengine (Phase 3.5 next adds the
    // per-word DMA path).  Drive with default (idle) inputs.
    d.disk = DiskIn::default();
    // Disk controller: driven by microengine's per-task disk_ctrl
    // outputs.  When the Disk Sector task asserts F1=DiskCtrlWrite,
    // the engine emits write_en + write_data targeting register
    // RSEL[2:0]; otherwise idle.  The engine outputs are q-registered
    // (1-cycle late from the firing instruction).
    d.disk_ctrl = CtrlIn {
        reg_addr: q.engine.disk_ctrl_addr,
        write_data: q.engine.disk_ctrl_write_data,
        write_en: q.engine.disk_ctrl_write_en,
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
            f1: F1Function::Constant, f2: F2Function::LoadMar,
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
            f1: F1Function::Constant, f2: F2Function::LoadMar,
            t_load: false, l_load: false, next: bits::<10>(1),
        };
        // addr 1: F1=Constant idx 1 = 0xBEEF + F2=WriteMd → memory[0x0100] ← 0xBEEF
        //         (RSEL=0, BS=1 → constant index = 1)
        let mi1 = Microinstruction {
            rsel: bits::<5>(0), aluf: AluFunction::Bus, bs: BusSource::LoadR, // BS=1
            f1: F1Function::Constant, f2: F2Function::WriteMd,
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
        // Microcode at addr 0: NOP loop (no L_LOAD; just sit there).
        // Both tasks share this microcode at addr 0 since they both
        // start from MPC=0 (reset value).
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = ui(0, AluFunction::Bus, BusSource::ReadR, false, false, 0);
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

    /// Per-task gating: under Disk Sector (Task 4), F1=DiskCtrlWrite
    /// asserts disk_ctrl_write_en; under Emulator (Task 0), the
    /// same instruction is a no-op.
    #[test]
    fn f1_disk_ctrl_write_only_active_under_disk_sector_task() {
        // Microcode at addr 0: F1=DiskCtrlWrite, RSEL=0, BS=ReadR.
        // (Both bus value and reg-index are 0; we only care about
        // the write_en signal.)
        let mi0 = Microinstruction {
            rsel: bits::<5>(0),
            aluf: AluFunction::Bus, bs: BusSource::ReadR,
            f1: F1Function::DiskCtrlWrite, f2: F2Function::Nop,
            t_load: false, l_load: false, next: bits::<10>(0),
        };
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = mi0.pack();
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
            "Under Disk Sector task, F1=DiskCtrlWrite should assert write_en");

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
            "Under Emulator task, F1=DiskCtrlWrite is a no-op (gated)");
    }

    /// End-to-end disk → wakeup → arbiter → engine path: the disk
    /// widget's sector_mark fires every 256 cycles; with a long-enough
    /// run, the Disk Sector task (Task 4) fires at least once.
    #[test]
    fn disk_sector_mark_fires_disk_sector_task() {
        // Microcode at addr 0: NOP loop.
        let mut microcode = [0u32; crate::microcode_rom::MICROCODE_WORDS];
        microcode[0] = ui(0, AluFunction::Bus, BusSource::ReadR, false, false, 0);
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

        // Sector_mark count.
        let sector_marks: u32 = trace.iter().filter(|t| t.disk_sector_mark).count() as u32;

        eprintln!("[boot_trace_baseline_metrics] 2000-cycle trace:");
        eprintln!("  distinct microaddresses visited: {}", visited.len());
        eprintln!("  task firing counts: {task_counts:?}");
        eprintln!("  disk sector_mark fired: {sector_marks} times");

        // Baseline assertions (intentionally loose):
        // - at least 1 distinct microaddress (the engine ran at all)
        // - sector_mark fires once per 256 cycles → ~7 marks in 2000 cycles
        assert!(visited.len() >= 1, "engine should have run at least one microinstruction");
        assert!(sector_marks >= 5,
            "disk sector_mark should fire ~7 times in 2000 cycles; saw {sector_marks}");
    }
}
