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
use rhdl_fpga::core::dff;

use crate::constant_rom::{ConstantIn, ConstantOut, ConstantRom};
use crate::microcode_rom::{MicrocodeRom, UromIn, UromOut};
use crate::microengine::{In as MicroIn, Microengine, Out as MicroOut};

/// Inputs to the AltoChip top-level.
///
/// Phase-3.5 minimum: there are no external inputs in the boot
/// scenario — disk and other peripherals are sub-widgets, and the
/// microcode is loaded at construction time.  Future phases will add
/// keyboard, mouse, etc. via this struct.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct ChipIn {
    /// Reserved for future use (keyboard, mouse, ethernet inputs).
    pub _placeholder: bool,
}

/// Outputs from the AltoChip top-level — observable for trace + tests.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct ChipOut {
    /// MPC the engine processed this cycle (echoed from microengine.o.mpc).
    pub mpc: Bits<10>,
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
}

/// The Alto chip — composition of microengine + microcode RAM + constant ROM.
///
/// Construct with [`AltoChip::with_microcode`] to load microcode and
/// constant images, or [`AltoChip::default`] for all-zero ROMs.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct AltoChip {
    /// Current MPC.  AltoChip owns this DFF; microengine reads it via
    /// input each cycle and returns next_mpc as output.
    mpc: dff::DFF<Bits<10>>,
    /// 2K-entry microcode RAM (loaded at construction).
    urom: MicrocodeRom,
    /// 256-entry constant ROM — read combinationally each cycle by
    /// the microengine via `F1=Constant`.
    crom: ConstantRom,
    /// Universal-task microengine.
    engine: Microengine,
}

impl Default for AltoChip {
    fn default() -> Self {
        Self {
            mpc: dff::DFF::new(bits::<10>(0)),
            urom: MicrocodeRom::default(),
            crom: ConstantRom::default(),
            engine: Microengine::default(),
        }
    }
}

impl AltoChip {
    /// Construct an AltoChip with the supplied microcode image but
    /// an empty constant ROM.  Useful for tests that don't depend on
    /// constants.
    pub fn with_microcode(microcode_words: &[u32; crate::microcode_rom::MICROCODE_WORDS]) -> Self {
        Self {
            mpc: dff::DFF::new(bits::<10>(0)),
            urom: MicrocodeRom::with_words(microcode_words),
            crom: ConstantRom::default(),
            engine: Microengine::default(),
        }
    }

    /// Construct an AltoChip with both microcode and constant ROMs
    /// loaded.  This is the normal Phase 3.5 boot constructor.
    pub fn with_microcode_and_constants(
        microcode_words: &[u32; crate::microcode_rom::MICROCODE_WORDS],
        constants: &[u16; crate::constant_rom::NUM_CONSTANTS],
    ) -> Self {
        Self {
            mpc: dff::DFF::new(bits::<10>(0)),
            urom: MicrocodeRom::with_words(microcode_words),
            crom: ConstantRom::with_constants(constants),
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
pub fn alto_chip_kernel(_cr: ClockReset, _i: ChipIn, q: Q) -> (ChipOut, D) {
    let mut d = D::dont_care();
    let mut o = ChipOut::dont_care();

    // Microcode RAM has 1-cycle BRAM latency.  q.urom.instruction is
    // the instruction at the address presented one cycle ago.  We
    // present q.mpc (current cycle's MPC) for the next cycle's read.
    //
    // Wait — that's a 1-cycle bubble.  Properly: at cycle T we want
    // the engine to process the instr at mpc(T).  That instr was
    // fetched by presenting mpc(T) at cycle T-1.  So d.urom.read_addr
    // at cycle T-1 was mpc(T) = next_mpc(T-1).
    //
    // Today we set d.urom.read_addr = next_mpc this cycle, so at
    // cycle T+1 the BRAM has contents[next_mpc(T)] = contents[mpc(T+1)].
    let instr_this_cycle: Bits<32> = q.urom.instruction;

    // Decode RSEL[31:27] and BS[22:20] from the instruction; constant
    // ROM index = (RSEL << 3) | BS = 8 bits.  Drive the constant ROM
    // combinationally with this index so the engine sees the looked-up
    // value the same cycle it processes the instruction.
    let rsel: Bits<5> = ((instr_this_cycle >> 27) & bits::<32>(0x1F)).resize();
    let bs:   Bits<3> = ((instr_this_cycle >> 20) & bits::<32>(0x07)).resize();
    let const_idx: Bits<8> = (rsel.resize::<8>() << 3) | bs.resize::<8>();
    d.crom = ConstantIn { index: const_idx };
    let const_value: Bits<16> = q.crom.value;

    d.engine = MicroIn {
        mpc: q.mpc,
        instr: instr_this_cycle,
        constant_value: const_value,
    };

    // Drive next cycle's instruction fetch from microengine's computed
    // next_mpc.  Engine output is combinational from its inputs.
    // Use q.engine to read the engine's output (registered output of
    // the previous cycle's evaluation).
    let next_mpc: Bits<10> = q.engine.next_mpc;
    d.urom = UromIn { mpc: next_mpc.resize() };

    // Commit MPC: the next-cycle MPC is the engine's computed next_mpc.
    d.mpc = next_mpc;

    // Outputs
    o.mpc          = q.mpc;
    o.next_mpc     = next_mpc;
    o.t            = q.engine.t;
    o.l            = q.engine.l;
    o.bus          = q.engine.bus;
    o.alu_result   = q.engine.alu_result;
    o.instruction  = instr_this_cycle;

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

    fn run(uut: AltoChip, cycles: usize) -> Vec<ChipOut> {
        let inputs: Vec<ChipIn> = (0..cycles).map(|_| ChipIn::default()).collect();
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
        let inputs: Vec<ChipIn> = (0..8).map(|_| ChipIn::default()).collect();
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
}
