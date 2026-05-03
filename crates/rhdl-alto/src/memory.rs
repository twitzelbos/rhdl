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
use rhdl_fpga::core::dff;
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
    /// True when this cycle's microinstruction has F1=LoadMar (= MAR<-).
    /// Used by the pipeline-stall FSM to (a) reset cycles_since_mar to
    /// the "MAR<- cycle" value when allowed and (b) detect "back-to-back
    /// MAR<- too soon" stall conditions.  Per AltoHW §2.3 + spec digest:
    /// MAR<- starts a memory cycle.  A new MAR<- before the previous
    /// memory cycle has cleared the bus stalls the microengine.
    pub mar_load_this_cycle: bool,
    /// True when this cycle's microinstruction sources MD onto the bus
    /// (= BS=MemoryData / `←MD`).  Per Alto II timing: the read result
    /// is available 4 cycles after MAR<-; ←MD attempted before then
    /// stalls.
    pub md_read_this_cycle: bool,
    /// True when this cycle's microinstruction asserts F2=StoreMd
    /// (= MD<-).  Per AltoHW §2.3: there must be ≥1 intervening cycle
    /// between MAR<- and MD<-; MD<- attempted in the immediately-
    /// adjacent cycle stalls.
    pub md_write_this_cycle: bool,
}

/// Outputs from the memory subsystem.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct MemOut {
    /// Word read from the address presented one cycle ago.  BRAM-style
    /// 1-cycle latency.
    pub read_data: Bits<16>,
    /// True when the microengine MUST freeze (don't advance MPC, don't
    /// update DFFs) for this cycle because the requested memory access
    /// (←MD or MD<-) or new MAR<- conflicts with an in-flight memory
    /// pipeline.  See [`MemIn`] field comments for the precise rules.
    pub mem_stall: bool,
}

/// 64 KW × 16-bit memory subsystem with the **Alto II memory-pipeline
/// stall FSM** (per AltoHW §2.3 + spec digest §3 timing rules,
/// cross-validated against observed ContrAlto cycle counts on KSEC
/// boot at MPC=0x385 and 0x389).
///
/// ## Pipeline FSM
///
/// `cycles_since_mar` counts elapsed cycles since the most recent
/// successful MAR<-.  Initial value 7 (a "bus is idle" sentinel — any
/// value ≥ 5 suffices).  On a successful MAR<- this cycle, the FSM
/// latches `1` for next cycle (= "MAR<- was 1 cycle ago" at K+1).
/// Each subsequent non-stalled cycle increments the counter, capped
/// at 7.
///
/// **Stall conditions** (`counter` = `q.cycles_since_mar`):
///
/// - `←MD` (read) requires `counter ≥ 4` (Alto II read result
///   available on the 4th cycle after MAR<-).  Stalls when
///   `1 ≤ counter ≤ 3`.
/// - `MD<-` (write) requires `counter ≥ 2` (≥ 1 intervening cycle
///   per AltoHW §2.3 (a)).  Stalls when `counter == 1`.
/// - New `MAR<-` requires `counter == 0` (idle) OR `counter ≥ 4`.
///   Stalls when `1 ≤ counter ≤ 3`.  Per AltoHW §2.3 + Alto II
///   timing: a memory cycle completes at K+4 (read) or K+3 (write);
///   the bus is free for a new MAR<- on the cycle of completion.
///   Cross-validated against ContrAlto: KSEC's first instruction
///   (MPC=0x004 = MAR<-) executes at exactly K+4 of Emulator's NOVEM
///   MAR<- (counter=4), and ContrAlto does NOT stall there — confirming
///   the K+4 threshold (not K+5).
///
/// When stalled, the FSM **does not freeze** — the memory pipeline
/// keeps ticking (the in-flight cycle continues to completion in real
/// hardware); only the microengine freezes via `o.mem_stall`.  When
/// non-stalled and MAR<- fires this cycle, the counter latches `1`
/// for next cycle.  Otherwise the counter increments (capped at 7).
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct Memory {
    /// 64K × 16-bit BRAM.
    bram: SyncBRAM<Bits<16>, 16>,
    /// Cycles elapsed since the most recent MAR<- (capped at 7).
    /// 0 = sentinel "no in-flight memory cycle".
    /// 1 = MAR<- happened this cycle.
    /// 2..=4 = in-flight (memory cycle still running).
    /// 5+ = read result valid (Alto II latches; ←MD allowed any time).
    /// Initialized to 7 (idle) so the first MAR<- can issue
    /// immediately without spurious stall.
    cycles_since_mar: dff::DFF<Bits<3>>,
}

impl Default for Memory {
    fn default() -> Self {
        Self {
            bram: SyncBRAM::default(),
            cycles_since_mar: dff::DFF::new(bits::<3>(7)),
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
            cycles_since_mar: dff::DFF::new(bits::<3>(7)),
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
pub fn memory_kernel(cr: ClockReset, i: MemIn, q: Q) -> (MemOut, D) {
    let mut d = D::dont_care();
    let mut o = MemOut::dont_care();

    // ---- Pipeline-stall FSM (per AltoHW §2.3 + spec digest §3) -----
    //
    // `q.cycles_since_mar` ∈ {0..7}.  Sentinel 7 = "bus idle, no
    // in-flight memory cycle".  After a successful MAR<-, FSM resets
    // to 1 and increments each non-stalled cycle.
    //
    // Stall rules:
    //   ←MD     stalls when 1 ≤ counter ≤ 4 (read result valid at
    //           counter ≥ 5 — Alto II 4th-cycle availability).
    //   MD<-    stalls when counter == 1 (need ≥ 1 intervening cycle
    //           per spec).
    //   New MAR<- stalls when 1 ≤ counter ≤ 2 (need ≥ 2 intervening
    //           cycles for back-to-back MAR<-).
    // Counter encoding (latches at end of cycle):
    //   counter ∈ {1,2,3,...} measures "cycles elapsed since the most
    //   recent successful MAR<-" — counter=1 at K+1 (one cycle after
    //   MAR<- on cycle K), counter=2 at K+2, etc.  counter=7 is the
    //   idle sentinel.
    //
    // Stall thresholds (per AltoHW §2.3 + spec digest §3 + observed
    // ContrAlto cycle counts on KSEC boot at MPC=0x385 and 0x389):
    //   ←MD       requires counter ≥ 4 (= K+4, Alto II 4th-cycle read).
    //               Stalls when 1 ≤ counter ≤ 3.
    //   MD<-      requires counter ≥ 2 (= K+2, "1 minimum intervening
    //               microinstruction" rule).  Stalls when counter == 1.
    //   New MAR<- requires counter == 0 (idle) OR counter ≥ 5.
    //               Stalls when 1 ≤ counter ≤ 4.  This is conservative
    //               (= treat every previous cycle as if it were a read),
    //               matching observed CTR cycle-count for back-to-back
    //               MAR<- separated by an MD<-.
    let counter: Bits<3> = q.cycles_since_mar;
    let counter_ge_1: bool = counter >= bits::<3>(1);
    let stall_read: bool =
        i.md_read_this_cycle && counter_ge_1 && counter < bits::<3>(4);
    // MD<- (write) does NOT stall the microengine.  Per AltoHW §2.3:
    // "Store happens in the third cycle after MAR<-" — the microcode
    // can issue MD<- at K+1, K+2, or K+3 and the write commits at K+3
    // regardless.  The MD<- microinstruction just LATCHES the data;
    // it doesn't wait for the bus.  Confirmed by canonical
    // altoIIcode3.mu which does NOVEM (MAR<-) → INXB (MD<-) back-
    // to-back at MPC 0 → 0x152 (= K, K+1 with 0 intervening cycles).
    // The earlier "1 minimum intervening" reading of rule (a) was too
    // strict; the constraint applies to ←MD (read) where the result
    // must be ready, not to MD<- (write) which the bus completes
    // asynchronously at K+3.
    let stall_write: bool = false;
    let _ = i.md_write_this_cycle;  // accepted for the FSM driver-signal
                                    // contract but unused here
    // Per AltoHW §2.3 + Alto II "Read result available in cycle four":
    // the memory cycle COMPLETES at K+4 for a read (K+3 for a write).
    // The bus is free for a new MAR<- on the SAME cycle the previous
    // cycle completes — so new MAR<- requires counter ≥ 4.  This
    // matches observed ContrAlto behavior on KSEC's first instruction
    // (MPC=0x004 issues MAR<- exactly 4 cycles after Emulator's
    // NOVEM MAR<-; CTR doesn't stall there).  Earlier I used counter
    // ≥ 5, which spuriously stalled KSEC's first instruction.
    let stall_new_mar: bool =
        i.mar_load_this_cycle && counter_ge_1 && counter < bits::<3>(4);
    let stall: bool = stall_read || stall_write || stall_new_mar;
    o.mem_stall = stall;

    // ---- Counter advancement ---------------------------------------
    //
    // Whether stalled or not, the memory pipeline keeps ticking
    // (the in-flight cycle continues to completion in the real
    // hardware).  But:
    //   - When MAR<- successfully fires this cycle (= MAR<- bit is
    //     set AND not stalled), reset counter to 1.
    //   - Otherwise, increment counter (capped at 7).
    let mar_fires_now: bool = i.mar_load_this_cycle && !stall;
    let next_counter: Bits<3> = if mar_fires_now {
        bits::<3>(1)
    } else if counter < bits::<3>(7) {
        counter + bits::<3>(1)
    } else {
        bits::<3>(7)
    };
    d.cycles_since_mar = next_counter;

    // ---- BRAM access ------------------------------------------------
    //
    // The BRAM read/write are driven from `i.address` / `i.write_en`
    // unconditionally — the stall only affects the MICROENGINE side.
    // Memory itself keeps running.  The microengine is responsible for
    // not advancing its own state when stalled, so its `i.address` and
    // `i.write_data` remain stable across stall cycles (= same access
    // re-issued — idempotent against BRAM).
    d.bram = BramIn::<Bits<16>, 16> {
        read_addr: i.address,
        write: BramWrite::<Bits<16>, 16> {
            addr: i.address,
            value: i.write_data,
            enable: i.write_en,
        },
    };

    o.read_data = q.bram;

    // Reset semantics: counter goes back to idle sentinel.
    if cr.reset.any() {
        d.cycles_since_mar = bits::<3>(7);
        o.mem_stall = false;
    }

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
            MemIn { address: b16(0x100), write_data: b16(0xABCD), write_en: true, ..Default::default() },
            // Cycle 1: present read addr (write committed at end of cycle 0).
            MemIn { address: b16(0x100), write_data: b16(0), write_en: false, ..Default::default() },
            // Cycle 2: BRAM output reflects the write.
            MemIn { address: b16(0x100), write_data: b16(0), write_en: false, ..Default::default() },
            MemIn { address: b16(0x100), write_data: b16(0), write_en: false, ..Default::default() },
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
            MemIn { address: b16(0x10), write_data: b16(0), write_en: false, ..Default::default() },
            MemIn { address: b16(0x20), write_data: b16(0), write_en: false, ..Default::default() },
            MemIn { address: b16(0x30), write_data: b16(0), write_en: false, ..Default::default() },
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
            MemIn { address: b16(0x80), write_data: b16(0), write_en: false, ..Default::default() },
            MemIn { address: b16(0x81), write_data: b16(0), write_en: false, ..Default::default() },
            MemIn { address: b16(0x82), write_data: b16(0), write_en: false, ..Default::default() },
            MemIn { address: b16(0x83), write_data: b16(0), write_en: false, ..Default::default() },
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
            MemIn { address: b16(0x10), write_data: b16(0x10), write_en: true, ..Default::default() },
            MemIn { address: b16(0x20), write_data: b16(0x20), write_en: true, ..Default::default() },
            MemIn { address: b16(0x10), write_data: b16(0), write_en: false, ..Default::default() },
            MemIn { address: b16(0x20), write_data: b16(0), write_en: false, ..Default::default() },
            MemIn { address: b16(0x30), write_data: b16(0), write_en: false, ..Default::default() }, // unwritten
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
            MemIn { address: b16(0xFFFF), write_data: b16(0xDEAD), write_en: true, ..Default::default() },
            MemIn { address: b16(0xFFFF), write_data: b16(0), write_en: false, ..Default::default() },
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
            ..Default::default()
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
