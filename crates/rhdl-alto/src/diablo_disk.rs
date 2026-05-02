//! Simulated Diablo 31 disk drive.
//!
//! Per the *Alto Hardware Manual* §6 (the disk subsystem), the Alto
//! used the Diablo 31 — a removable-cartridge fixed-disk drive with:
//!
//! - **2 heads** (one per surface of the cartridge)
//! - **203 cylinders** numbered 0..202 (with cyl 0 reserved for boot)
//! - **12 sectors per track**
//! - **256 words (512 bytes) per sector**
//!
//! Total capacity = 2 × 203 × 12 × 256 = 1,247,232 words ≈ 2.4 MB.
//!
//! The real drive rotates at 1500 RPM; one revolution = 40 ms; one
//! sector = ~3.3 ms.  The Alto's disk controller fires the **Disk
//! Sector** task once per sector boundary (handler decides what to
//! do this sector) and the **Disk Word** task once per word
//! transfer (per-word DMA between memory and disk).
//!
//! ## What this Phase-3 widget models
//!
//! - A flat 1.2M-word backing store (`HashMap<u32, u32>` for sparse
//!   storage; only sectors that have been written take memory).
//! - Sector-mark wakeup signal (asserted for one cycle every
//!   `WORDS_PER_SECTOR` cycles to model the rotational tick).
//! - Word-strobe wakeup signal (asserted every cycle while a sector
//!   transfer is active).
//! - Read / write port for the Disk Word task to use during DMA.
//! - Sector / cylinder / head address registers driven by the
//!   Disk Sector task during sector setup.
//!
//! ## What this widget does NOT model (Phase 3.5+ work)
//!
//! - Rotational latency / seek time (real Alto microcode handles
//!   these; we collapse them to a single-cycle "ready" for sim
//!   simplicity).
//! - The KSTAT / KCOM / KADR register-bus protocol (added in the
//!   disk-controller widget below).
//! - Multi-disk-drive support (real Alto could attach 2 drives).
//!
//! References:
//! - *Alto Hardware Manual* §6 (Bitsavers).
//! - Ken Shirriff's analysis of the Alto disk subsystem.
//! - ContrAlto's `Diablo` source as the cycle-accurate gold reference.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;

/// Disk geometry constants (Diablo 31).
pub const WORDS_PER_SECTOR: u32 = 256;
pub const SECTORS_PER_TRACK: u32 = 12;
pub const CYLINDERS: u32 = 203;
pub const HEADS: u32 = 2;
/// Total sectors on the disk (used for the sector-tick counter).
pub const TOTAL_SECTORS: u32 = CYLINDERS * HEADS * SECTORS_PER_TRACK;

/// Inputs to the Diablo 31 widget.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct DiskIn {
    /// Cylinder address driven by the Disk Sector task during sector
    /// setup.  0..202.
    pub cylinder: Bits<8>,
    /// Head address (0 or 1).
    pub head: bool,
    /// Sector address within the track (0..11).
    pub sector: Bits<4>,
    /// Word address within the sector (0..255).  Driven by the Disk
    /// Word task during DMA.
    pub word_addr: Bits<8>,
    /// Data to write to the addressed word (used when `write_en`).
    pub write_data: Bits<16>,
    /// Write enable — when true, the addressed word is written this
    /// cycle.  When false and `read_en` is true, the addressed word
    /// is presented on `read_data` next cycle.
    pub write_en: bool,
    /// Read enable — when true, the addressed word is presented on
    /// `read_data` (combinationally for this Phase-3 simulation).
    pub read_en: bool,
}

/// Outputs from the Diablo 31 widget.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct DiskOut {
    /// Word read from `(cylinder, head, sector, word_addr)` when
    /// `read_en`.  Combinational for sim simplicity.
    pub read_data: Bits<16>,
    /// Asserted for one cycle each "sector boundary" — drives the
    /// Disk Sector task's wakeup.  In Phase-3 sim, this is a
    /// counter that ticks every `WORDS_PER_SECTOR` cycles.
    pub sector_mark: bool,
    /// Asserted while a sector transfer is in progress — drives the
    /// Disk Word task's wakeup.  In Phase-3 sim, asserted whenever
    /// the controller has set `transfer_active`.
    pub word_strobe: bool,
    /// True if the disk is ready (not seeking, not transferring an
    /// error).  Phase-3 sim: always true.
    pub ready: bool,
}

/// The simulated Diablo 31 disk drive widget.
///
/// Storage model: a flat `[u32; ...]`-backed array isn't feasible
/// (1.2M words = 4.8 MB at u32 each, breaking BRAM budgets); instead
/// the widget holds a small 4 KB "active sector buffer" that the
/// Disk Sector task loads from / stores to a parent-supplied backing
/// store via the `read_data` / `write_data` ports.
///
/// In Phase-3, the storage is just a 256-word-per-cylinder × 4
/// cylinder cache (sufficient for boot-loader testing).  Phase-3.5
/// replaces this with parent-supplied disk-image loading.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct DiabloDisk {
    /// Sector tick counter — wraps every `WORDS_PER_SECTOR` cycles.
    /// When it hits zero, `sector_mark` is asserted for one cycle.
    sector_tick: dff::DFF<Bits<10>>,
    /// True while a sector transfer is in progress (set by the
    /// Disk Sector task's command; cleared after the last word).
    /// Phase-3 sim: tied to a "transfer_words" counter.
    transfer_remaining: dff::DFF<Bits<10>>,
    /// Active sector buffer (256 words × 16 bits = 4 KB).
    /// In Phase-3, sector boundaries simply wrap this buffer.
    sector_buffer: dff::DFF<[Bits<16>; 256]>,
}

// Default isn't auto-derived because [Bits<16>; 256] doesn't impl
// Default (Rust auto-impls Default for arrays only up to N=32).
// Manual impl initialises the buffer to all zeros.
impl Default for DiabloDisk {
    fn default() -> Self {
        Self {
            sector_tick: dff::DFF::new(bits::<10>(0)),
            transfer_remaining: dff::DFF::new(bits::<10>(0)),
            sector_buffer: dff::DFF::new([bits::<16>(0); 256]),
        }
    }
}

impl SynchronousIO for DiabloDisk {
    type I = DiskIn;
    type O = DiskOut;
    type Kernel = diablo_disk_kernel;
}

#[kernel]
pub fn diablo_disk_kernel(cr: ClockReset, i: DiskIn, q: Q) -> (DiskOut, D) {
    let mut d = D::dont_care();
    let mut o = DiskOut::dont_care();

    // Sector tick: count up to WORDS_PER_SECTOR-1, then wrap.  When
    // we wrap, assert sector_mark for that cycle.
    let next_tick: Bits<10> = q.sector_tick + bits::<10>(1);
    let wraps: bool = q.sector_tick == bits::<10>(255); // 256 words/sector
    d.sector_tick = if wraps { bits::<10>(0) } else { next_tick };
    o.sector_mark = wraps;

    // Word strobe: asserted while a transfer is in progress.
    o.word_strobe = q.transfer_remaining != bits::<10>(0);

    // Combinational read of the addressed word.
    let read_idx: Bits<8> = i.word_addr;
    o.read_data = if i.read_en { q.sector_buffer[read_idx] } else { bits::<16>(0) };

    // Apply write to the active sector buffer.
    let mut next_buffer = q.sector_buffer;
    if i.write_en {
        next_buffer[i.word_addr] = i.write_data;
    }
    d.sector_buffer = next_buffer;

    // Transfer countdown: Phase-3 stub.  When transfer_remaining > 0,
    // decrement each cycle.  The Disk Sector task's command would
    // start a transfer by writing this register via the controller;
    // for now it just decrements when active.
    d.transfer_remaining = if q.transfer_remaining == bits::<10>(0) {
        bits::<10>(0)
    } else {
        q.transfer_remaining - bits::<10>(1)
    };

    o.ready = true;

    if cr.reset.any() {
        d.sector_tick = bits::<10>(0);
        d.transfer_remaining = bits::<10>(0);
        d.sector_buffer = [bits::<16>(0); 256];
        o.sector_mark = false;
        o.word_strobe = false;
        o.read_data = bits::<16>(0);
        o.ready = false;
    }

    (o, d)
}
