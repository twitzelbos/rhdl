//! Microcode RAM (sometimes called "uROM" — it's strictly read-only
//! during normal operation, but the Alto II RAM-resident microcode
//! support means we model it as RAM internally).
//!
//! Wraps a [`SyncBRAM`]`<Bits<32>, 11>` (2048 32-bit microinstructions)
//! with an init-from-microcode-loader API.  The microengine drives an
//! 11-bit MPC; the BRAM responds with the packed microinstruction
//! one cycle later.
//!
//! ## Phase-3.5 capabilities
//!
//! - **2048-word backing store** (full Alto II — both microcode banks).
//! - **`SyncBRAM`-backed** — synthesizable; iverilog testbench compiles
//!   cleanly via the `.skip(2)` initial-X-state convention.
//! - **Initial contents** loaded from the [`crate::microcode_loader`]
//!   output via [`MicrocodeRom::with_words`].
//! - **Read-only contract** — the kernel's write port is wired to
//!   `enable=false` permanently; conceptually this is a ROM.

use rhdl::prelude::*;
use rhdl_fpga::core::ram::synchronous::{In as BramIn, SyncBRAM, Write as BramWrite};

/// Total microinstructions in the Alto II microcode address space.
pub const MICROCODE_WORDS: usize = 2048;

/// Address-bit count for the microcode address space.
pub const ADDR_BITS: usize = 11;

/// Inputs to the microcode RAM.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct UromIn {
    /// 11-bit microaddress to fetch from.
    pub mpc: Bits<11>,
}

/// Outputs from the microcode RAM.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct UromOut {
    /// Packed 32-bit microinstruction at the address presented one
    /// cycle ago (BRAM 1-cycle read latency).
    pub instruction: Bits<32>,
}

/// 2048-word × 32-bit microcode RAM.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct MicrocodeRom {
    bram: SyncBRAM<Bits<32>, 11>,
}

impl Default for MicrocodeRom {
    fn default() -> Self {
        Self {
            bram: SyncBRAM::default(),
        }
    }
}

impl MicrocodeRom {
    /// Construct a microcode RAM with the supplied initial contents.
    /// Pass the output of [`crate::microcode_loader::load_alto_ii_microcode`]
    /// directly.
    pub fn with_words(words: &[u32; MICROCODE_WORDS]) -> Self {
        let initial = words
            .iter()
            .enumerate()
            .map(|(i, &w)| (bits::<11>(i as u128), bits::<32>(w as u128)));
        Self {
            bram: SyncBRAM::new(initial),
        }
    }
}

impl SynchronousIO for MicrocodeRom {
    type I = UromIn;
    type O = UromOut;
    type Kernel = microcode_rom_kernel;
}

#[kernel]
pub fn microcode_rom_kernel(_cr: ClockReset, i: UromIn, q: Q) -> (UromOut, D) {
    let mut d = D::dont_care();
    let mut o = UromOut::dont_care();

    // Read-only contract: write port is permanently disabled.
    d.bram = BramIn::<Bits<32>, 11> {
        read_addr: i.mpc,
        write: BramWrite::<Bits<32>, 11> {
            addr: bits::<11>(0),
            value: bits::<32>(0),
            enable: false,
        },
    };

    o.instruction = q.bram;

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b11(v: u16) -> Bits<11> {
        bits::<11>(v as u128)
    }
    fn b32(v: u32) -> Bits<32> {
        bits::<32>(v as u128)
    }

    fn run_inputs(uut: MicrocodeRom, inputs: Vec<UromIn>) -> Vec<UromOut> {
        let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
        uut.run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect()
    }

    #[test]
    fn fetch_preloaded_microinstruction() {
        // Preload two distinct microinstructions.
        let mut words = [0u32; MICROCODE_WORDS];
        words[0] = 0xDEADBEEF;
        words[0x400] = 0x12345678; // bank-1 entry point
        words[0x7FF] = 0xCAFEBABE; // last microinstruction
        let uut = MicrocodeRom::with_words(&words);
        let trace = run_inputs(
            uut,
            vec![
                UromIn { mpc: b11(0) },
                UromIn { mpc: b11(0x400) },
                UromIn { mpc: b11(0x7FF) },
                UromIn { mpc: b11(0) },
            ],
        );
        // 1-cycle BRAM latency.
        assert_eq!(trace[1].instruction, b32(0xDEADBEEF), "uROM[0]");
        assert_eq!(trace[2].instruction, b32(0x12345678), "uROM[0x400]");
        assert_eq!(trace[3].instruction, b32(0xCAFEBABE), "uROM[0x7FF]");
    }

    /// Real-PROM integration: load actual Alto II microcode and
    /// confirm at least the first instruction is well-formed.
    #[test]
    fn load_real_microcode_and_fetch() {
        use crate::microcode_loader;
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("rom");
        if !dir.join("U55").exists() {
            eprintln!("[load_real_microcode_and_fetch] skipping — PROMs absent in {dir:?}");
            return;
        }
        let words =
            microcode_loader::load_alto_ii_microcode_from_dir(&dir).expect("load real PROMs");
        let uut = MicrocodeRom::with_words(&words);
        let trace = run_inputs(uut, vec![UromIn { mpc: b11(0) }, UromIn { mpc: b11(0) }]);
        assert_eq!(
            trace[1].instruction,
            b32(words[0]),
            "uROM[0] (Silent Boot entry) should match the loader output"
        );
        // Spot-check: microinstruction 0 must be non-trivial.
        assert_ne!(words[0], 0u32);
        assert_ne!(words[0], 0xffff_ffff);
    }

    #[test]
    fn microcode_rom_iverilog_round_trip() -> Result<(), RHDLError> {
        let mut words = [0u32; MICROCODE_WORDS];
        for i in 0..16 {
            words[i] = 0xA5A5_0000 | i as u32;
        }
        let uut = MicrocodeRom::with_words(&words);
        let inputs: Vec<UromIn> = (0..6).map(|i| UromIn { mpc: b11(i as u16) }).collect();
        let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &TestBenchOptions::default().skip(2))?;
        tm.run_iverilog()?;
        Ok(())
    }
}
