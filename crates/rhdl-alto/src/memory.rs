//! Alto main memory — 64 KW × 16 bits backed by [`SyncBRAM`].
//!
//! Per the *Alto Hardware Manual* §3, the Alto has 64 KW of word-
//! addressable MOS memory.  The microengine drives an address each
//! cycle; reads complete one cycle later (BRAM-style latency, matching
//! both real Alto MOS-DRAM timing and the FPGA primitive).
//!
//! ## Phase-3.5 capabilities
//!
//! - **64 KW backing store** (16-bit address) — full Alto memory size.
//! - **`SyncBRAM`-backed** so the emitted Verilog uses a proper
//!   block-RAM primitive on synthesis (fits in a single Xilinx 7-series
//!   BRAM tile or equivalent), and `iverilog` testbench compilation
//!   stays tractable.
//! - **One read port + one write port** (single-cycle, 1-cycle latency).
//! - **Initial contents** can be supplied via [`Memory::new`] — this is
//!   how disk-image words are loaded when the boot loader pre-stages
//!   the boot block, and how the microcode bootstrap can load a
//!   reference image directly for testing.
//!
//! ## What this widget does NOT model (deferred to MRT integration)
//!
//! - Memory Reference Task (MRT) timing — the real Alto memory bus
//!   sequences reads/writes over multiple microcycles via STARTF /
//!   MAR← / MD→ / MD2L control.  Phase 3.5 leaves the MRT timing to
//!   the microengine; this widget exposes the simpler 1-cycle BRAM
//!   contract that the MRT sub-task drives.
//! - Memory parity / ECC.

use rhdl::prelude::*;
use rhdl_fpga::core::ram::synchronous::{In as BramIn, SyncBRAM, Write as BramWrite};

/// Memory size in words.  Phase 3.5 uses 64K (the real Alto's main
/// memory size).  Address bits = log2(MEMORY_WORDS) = 16.
pub const MEMORY_WORDS: usize = 65536;

/// Address-bit count.  Used as the const-generic on [`SyncBRAM`].
pub const ADDR_BITS: usize = 16;

/// Inputs to the memory subsystem.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct MemIn {
    /// Word address.
    pub address: Bits<16>,
    /// Data to write (when `write_en`).
    pub write_data: Bits<16>,
    /// Write enable.
    pub write_en: bool,
}

/// Outputs from the memory subsystem.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct MemOut {
    /// Word read from the address presented one cycle ago.  BRAM-style
    /// 1-cycle latency.
    pub read_data: Bits<16>,
}

/// 64 KW × 16-bit memory subsystem.
///
/// Construct with [`Memory::new`] to supply initial contents (boot
/// image, microcode-fetched data, etc.) or with [`Memory::default`]
/// for an all-zero memory.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct Memory {
    /// 64K × 16-bit BRAM.
    bram: SyncBRAM<Bits<16>, 16>,
}

impl Default for Memory {
    fn default() -> Self {
        Self {
            bram: SyncBRAM::default(),
        }
    }
}

impl Memory {
    /// Construct a memory with the supplied initial contents.
    ///
    /// `initial` is an iterator of `(address, value)` pairs.
    /// Addresses absent from the iterator default to 0.
    pub fn new(initial: impl IntoIterator<Item = (Bits<16>, Bits<16>)>) -> Self {
        Self {
            bram: SyncBRAM::new(initial),
        }
    }

    /// Convenience: load a contiguous run of words starting at a base
    /// address (typical for staging a disk sector's data).
    pub fn with_words_at(base: u16, words: &[u16]) -> Self {
        let initial = words.iter().enumerate().map(|(i, &w)| {
            (bits::<16>(base as u128 + i as u128), bits::<16>(w as u128))
        });
        Self::new(initial)
    }
}

impl SynchronousIO for Memory {
    type I = MemIn;
    type O = MemOut;
    type Kernel = memory_kernel;
}

#[kernel]
pub fn memory_kernel(_cr: ClockReset, i: MemIn, q: Q) -> (MemOut, D) {
    let mut d = D::dont_care();
    let mut o = MemOut::dont_care();

    // Drive the BRAM's read address from this cycle's input; the
    // BRAM's output appears one cycle later.
    d.bram = BramIn::<Bits<16>, 16> {
        read_addr: i.address,
        write: BramWrite::<Bits<16>, 16> {
            addr: i.address,
            value: i.write_data,
            enable: i.write_en,
        },
    };

    o.read_data = q.bram;

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b16(v: u16) -> Bits<16> { bits::<16>(v as u128) }

    fn run_inputs(uut: Memory, inputs: Vec<MemIn>) -> Vec<MemOut> {
        let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
        uut.run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect()
    }

    /// Write at cycle 0, read-back at cycle 1 → expected at trace
    /// position 2 (BRAM has 1-cycle commit + 1-cycle read latency).
    #[test]
    fn write_then_read_round_trip() {
        let uut = Memory::default();
        let trace = run_inputs(uut, vec![
            // Cycle 0: write 0xABCD to addr 0x100.
            MemIn { address: b16(0x100), write_data: b16(0xABCD), write_en: true },
            // Cycle 1: present read addr (write committed at end of cycle 0).
            MemIn { address: b16(0x100), write_data: b16(0), write_en: false },
            // Cycle 2: BRAM output reflects the write.
            MemIn { address: b16(0x100), write_data: b16(0), write_en: false },
            MemIn { address: b16(0x100), write_data: b16(0), write_en: false },
        ]);
        // Read appears 2 cycles after the write input was supplied.
        assert_eq!(trace[2].read_data, b16(0xABCD), "read of 0x100 should yield 0xABCD");
    }

    #[test]
    fn initial_contents_via_new() {
        let uut = Memory::new(vec![
            (b16(0x10), b16(0xCAFE)),
            (b16(0x20), b16(0xBEEF)),
        ]);
        let trace = run_inputs(uut, vec![
            MemIn { address: b16(0x10), write_data: b16(0), write_en: false },
            MemIn { address: b16(0x20), write_data: b16(0), write_en: false },
            MemIn { address: b16(0x30), write_data: b16(0), write_en: false },
            MemIn::default(),
        ]);
        // Read appears 1 cycle after the address is presented.
        assert_eq!(trace[1].read_data, b16(0xCAFE), "preloaded 0x10");
        assert_eq!(trace[2].read_data, b16(0xBEEF), "preloaded 0x20");
        assert_eq!(trace[3].read_data, b16(0),      "unset 0x30 reads as 0");
    }

    #[test]
    fn with_words_at_loads_a_run() {
        // Stage a 4-word sequence at base 0x80.
        let uut = Memory::with_words_at(0x80, &[0x1111, 0x2222, 0x3333, 0x4444]);
        let trace = run_inputs(uut, vec![
            MemIn { address: b16(0x80), write_data: b16(0), write_en: false },
            MemIn { address: b16(0x81), write_data: b16(0), write_en: false },
            MemIn { address: b16(0x82), write_data: b16(0), write_en: false },
            MemIn { address: b16(0x83), write_data: b16(0), write_en: false },
            MemIn::default(),
        ]);
        assert_eq!(trace[1].read_data, b16(0x1111));
        assert_eq!(trace[2].read_data, b16(0x2222));
        assert_eq!(trace[3].read_data, b16(0x3333));
        assert_eq!(trace[4].read_data, b16(0x4444));
    }

    #[test]
    fn writes_are_independent_per_address() {
        let uut = Memory::default();
        let trace = run_inputs(uut, vec![
            MemIn { address: b16(0x10), write_data: b16(0x10), write_en: true },
            MemIn { address: b16(0x20), write_data: b16(0x20), write_en: true },
            MemIn { address: b16(0x10), write_data: b16(0), write_en: false },
            MemIn { address: b16(0x20), write_data: b16(0), write_en: false },
            MemIn { address: b16(0x30), write_data: b16(0), write_en: false }, // unwritten
            MemIn::default(),
            MemIn::default(),
        ]);
        assert_eq!(trace[3].read_data, b16(0x10), "0x10 should hold 0x10");
        assert_eq!(trace[4].read_data, b16(0x20), "0x20 should hold 0x20");
        assert_eq!(trace[5].read_data, b16(0),    "0x30 was never written");
    }

    #[test]
    fn full_64k_address_space() {
        // Verify the high address space is reachable: write to the very
        // last word (0xFFFF) and read it back.
        let uut = Memory::default();
        let trace = run_inputs(uut, vec![
            MemIn { address: b16(0xFFFF), write_data: b16(0xDEAD), write_en: true },
            MemIn { address: b16(0xFFFF), write_data: b16(0), write_en: false },
            MemIn::default(),
            MemIn::default(),
        ]);
        assert_eq!(trace[2].read_data, b16(0xDEAD),
            "the top of the 64K address space should be reachable");
    }

    /// iverilog round-trip — preloads addresses so the BRAM never
    /// returns X.  Real BRAMs start uninitialized; the Rust sim reports
    /// 0 for unwritten addresses while Verilog reports X, which would
    /// trip the testbench checker.
    #[test]
    fn memory_iverilog_round_trip() -> Result<(), RHDLError> {
        // Preload addresses 0..6 so every read in the input sequence
        // hits a defined cell.
        let uut = Memory::with_words_at(0, &[0xA000, 0xB000, 0xC000, 0xD000, 0xE000, 0xF000]);
        let inputs: Vec<MemIn> = (0..6).map(|i| MemIn {
            address: b16(i as u16),
            write_data: b16(i as u16 * 0x10),
            write_en: false,  // read-only — write port stays idle
        }).collect();
        let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        // .skip(2) is the canonical SyncBRAM pattern — see
        // rhdl-fpga/src/core/ram/synchronous.rs tests.  Real BRAM
        // initial state is X; first 2 cycles are uninitialized.
        let tm = test_bench.rtl(&uut, &TestBenchOptions::default().skip(2))?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &TestBenchOptions::default().skip(2))?;
        tm.run_iverilog()?;
        Ok(())
    }
}
