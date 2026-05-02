//! Alto main memory — 64 KW × 16 bits (Phase-3 stub).
//!
//! The real Alto had 64 KW of MOS memory (later expandable to 128 KW
//! with the "Alto II" upgrade).  Per the *Alto Hardware Manual* §3,
//! memory is word-addressable; the microengine drives an address
//! and read/write enable each cycle.
//!
//! ## Phase-3 scope
//!
//! - **256-word backing store** (sized for iverilog testbench feasibility;
//!   the real Alto boot ROM is ~512 words, so this is enough to demonstrate
//!   the DMA path but not yet enough to boot a full image).  Phase-3.5 will
//!   parameterize the size and back the store with proper BRAM via
//!   `rhdl_fpga::core::ram::SyncBRAM`.
//! - **One read port + one write port** (single-cycle).
//! - **Combinational read** (collapsed for sim simplicity).
//! - **Reset to all zeros**.
//!
//! ## What this widget does NOT model (Phase 3.5+)
//!
//! - DRAM refresh (real Alto has Task 4 dedicated to this; on
//!   FPGA BRAM there's nothing to refresh).
//! - The Memory Block Move (BLT) port — Task 9 talks to memory
//!   directly via a different path; not modelled in Phase 3.
//! - Memory parity / ECC.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;

/// Memory size in words.  Phase-3 uses 256; Phase-3.5 will
/// parameterize via SyncBRAM.
pub const MEMORY_WORDS: u32 = 256;

/// Inputs to the memory subsystem.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct MemIn {
    /// Word address.  Phase-3: 8-bit.
    pub address: Bits<8>,
    /// Data to write (when `write_en`).
    pub write_data: Bits<16>,
    /// Write enable.
    pub write_en: bool,
    /// Read enable (drives `read_data` combinationally).
    pub read_en: bool,
}

/// Outputs from the memory subsystem.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct MemOut {
    /// Word at `address` when `read_en` is true; zero otherwise.
    pub read_data: Bits<16>,
}

/// 256-word memory subsystem.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct Memory {
    /// 256 × 16 bits = 512 bytes.  Phase-3.5 will swap this for SyncBRAM.
    cells: dff::DFF<[Bits<16>; 256]>,
}

// Manual Default — Rust auto-Default doesn't extend to arrays
// larger than 32.
impl Default for Memory {
    fn default() -> Self {
        Self {
            cells: dff::DFF::new([bits::<16>(0); 256]),
        }
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

    // Combinational read.
    o.read_data = if i.read_en { q.cells[i.address] } else { bits::<16>(0) };

    // Write port — commit at next edge.
    let mut next_cells = q.cells;
    if i.write_en {
        next_cells[i.address] = i.write_data;
    }
    d.cells = next_cells;

    if cr.reset.any() {
        d.cells = [bits::<16>(0); 256];
        o.read_data = bits::<16>(0);
    }

    (o, d)
}
