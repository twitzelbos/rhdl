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
use rhdl_fpga::core::constant::Constant;
use rhdl_fpga::core::dff;

/// Disk geometry constants (Diablo 31).
pub const WORDS_PER_SECTOR: u32 = 256;
pub const SECTORS_PER_TRACK: u32 = 12;
pub const CYLINDERS: u32 = 203;
pub const HEADS: u32 = 2;
/// Total sectors on the disk (used for the sector-tick counter).
pub const TOTAL_SECTORS: u32 = CYLINDERS * HEADS * SECTORS_PER_TRACK;

/// Spec-correct sector cadence in microcycles, derived per spec §8.1
/// (Diablo 31): rotation = 40 ms, 12 sectors/track ⇒ 3.333 ms/sector;
/// at the 170 ns microcycle (5.88 MHz) that's 3.333 ms / 170 ns =
/// **19,608 microcycles per sector boundary**.  This is the value the
/// real hardware enforces, NOT a simulation shortcut.
pub const SECTOR_PERIOD_CYCLES: u32 = 19608;

/// Width of the `sector_tick` counter in bits.  15 is the smallest
/// width that holds `SECTOR_PERIOD_CYCLES = 19608` (2^14 = 16384 isn't
/// enough; 2^15 = 32768 is plenty).
pub const SECTOR_TICK_W: usize = 15;

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
    /// Transfer-start request from the disk controller.  When asserted
    /// (typically when the Disk Sector microcode writes KCOM with the
    /// "start transfer" bit set), the disk arms a 256-word transfer
    /// by setting `transfer_remaining`.  This makes `word_strobe`
    /// fire for the next 256 cycles, waking the Disk Word task per
    /// word for DMA.
    pub transfer_request: bool,
    /// "I just consumed the current word" — asserted by the engine
    /// when the Disk Word task does an F2=DiskWordTransfer DMA write.
    /// On consumed, the disk advances `current_word_position` and
    /// decrements `transfer_remaining`.  Decoupling the position
    /// advance from raw cycles (and instead coupling it to actual
    /// DMA) makes the timing match the engine's task-firing cadence,
    /// not the simulator's clock cadence.
    pub word_consumed: bool,
    /// True when the running microinstruction has F1=Block.  Per
    /// *Alto Hardware Manual* §2.4 + spec §5.5: BLOCK is a hardware
    /// convention by which the running task asks its associated
    /// device to deassert that device's wakeup signal.  The disk
    /// snoops this together with `current_task` and clears its
    /// sustained `sector_wake` (when current_task=4) or `word_wake`
    /// (when current_task=14).
    pub block_task: bool,
    /// Which task is currently running (0..15).  Used together with
    /// `block_task` to decide which of the disk's wakeup signals to
    /// clear on F1=Block.
    pub current_task: Bits<4>,
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
    /// Current rotational word position (0..255).  Auto-increments
    /// each cycle while `transfer_remaining > 0`; resets to 0 when
    /// `transfer_request` is asserted.
    pub current_word_position: Bits<8>,
    /// Word at the current rotational position — combinational read of
    /// `sector_buffer[current_word_position]`.  This is what the
    /// Disk Word microcode reads to perform per-word DMA into memory.
    pub current_word_data: Bits<16>,
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
    /// Sector tick counter — wraps every `sector_period_minus_1 + 1`
    /// cycles.  When it wraps, the SECTOR-task wakeup is set
    /// (sustained until cleared by F1=Block from current_task=4).
    /// Width is 15 bits to hold the spec-correct period of 19,608
    /// cycles per sector boundary.
    sector_tick: dff::DFF<Bits<SECTOR_TICK_W>>,
    /// Wrap value for `sector_tick` (= sector period in cycles minus 1).
    /// Stored as a Constant so it doesn't cost flip-flops in synthesis
    /// but can still be customised per-instance via the
    /// [`DiabloDisk::with_test_period`] family of constructors.
    /// Default is `SECTOR_PERIOD_CYCLES - 1` (= 19,607), matching the
    /// real Diablo 31 hardware per spec §8.1.
    sector_period_minus_1: Constant<Bits<SECTOR_TICK_W>>,
    /// **Sustained** SECTOR-task wakeup signal per *Alto Hardware
    /// Manual* §2.4 + spec §5.5.  Set when the rotational sector_tick
    /// wraps (sector boundary detected); cleared when the Disk Sector
    /// microcode (current_task=4) executes F1=Block.  This matches
    /// the real Alto's "wakeup signals are hardware-generated" model.
    sector_wake: dff::DFF<bool>,
    /// True while a sector transfer is in progress (set by the
    /// Disk Sector task's command; cleared after the last word).
    /// Phase-3 sim: tied to a "transfer_words" counter.
    transfer_remaining: dff::DFF<Bits<10>>,
    /// Current rotational word position within the active sector
    /// (0..255).  Resets to 0 when `transfer_request` arms a fresh
    /// transfer; auto-increments per cycle while transfer active.
    current_word_position: dff::DFF<Bits<8>>,
    /// Active sector buffer (256 words × 16 bits = 4 KB).
    /// In Phase-3, sector boundaries simply wrap this buffer.
    sector_buffer: dff::DFF<[Bits<16>; 256]>,
}

// Default isn't auto-derived because [Bits<16>; 256] doesn't impl
// Default (Rust auto-impls Default for arrays only up to N=32).
// Manual impl initialises the buffer to all zeros.
/// Spec-correct wrap value: period - 1.
const DEFAULT_SECTOR_TICK_WRAP: u128 = (SECTOR_PERIOD_CYCLES - 1) as u128;

impl Default for DiabloDisk {
    fn default() -> Self {
        Self {
            sector_tick: dff::DFF::new(bits::<SECTOR_TICK_W>(0)),
            sector_period_minus_1: Constant::new(bits::<SECTOR_TICK_W>(DEFAULT_SECTOR_TICK_WRAP)),
            sector_wake: dff::DFF::new(false),
            transfer_remaining: dff::DFF::new(bits::<10>(0)),
            current_word_position: dff::DFF::new(bits::<8>(0)),
            sector_buffer: dff::DFF::new([bits::<16>(0); 256]),
        }
    }
}

impl DiabloDisk {
    /// Construct a DiabloDisk with the active sector buffer pre-loaded
    /// from the supplied 256-word array.  Useful for testing the DMA
    /// path without going through the disk-image-loader chain — and
    /// for staging a single sector worth of test data.  Sector period
    /// is the spec-correct 19,608 microcycles.
    pub fn with_sector(words: &[u16; 256]) -> Self {
        let mut buf = [bits::<16>(0); 256];
        for (i, &w) in words.iter().enumerate() {
            buf[i] = bits::<16>(w as u128);
        }
        Self {
            sector_tick: dff::DFF::new(bits::<SECTOR_TICK_W>(0)),
            sector_period_minus_1: Constant::new(bits::<SECTOR_TICK_W>(DEFAULT_SECTOR_TICK_WRAP)),
            sector_wake: dff::DFF::new(false),
            transfer_remaining: dff::DFF::new(bits::<10>(0)),
            current_word_position: dff::DFF::new(bits::<8>(0)),
            sector_buffer: dff::DFF::new(buf),
        }
    }

    /// Construct a DiabloDisk for the boot scenario: pre-loaded buffer
    /// PLUS `sector_tick = period_minus_1` so the very first cycle's
    /// wrap-check fires sector_mark immediately.  Matches ContrAlto's
    /// DiskController.cs which schedules the first SectorCallback at
    /// time 0.  Without this, the chip waits a full sector period
    /// (~19,608 cycles) before firing the first sector_mark, causing
    /// a multi-cycle delay before the boot dance can hand off to KSEC.
    /// Steady-state period is unchanged (spec-correct 19,608 cycles).
    pub fn with_sector_at_boundary(words: &[u16; 256]) -> Self {
        let mut buf = [bits::<16>(0); 256];
        for (i, &w) in words.iter().enumerate() {
            buf[i] = bits::<16>(w as u128);
        }
        Self {
            sector_tick: dff::DFF::new(bits::<SECTOR_TICK_W>(DEFAULT_SECTOR_TICK_WRAP)),
            sector_period_minus_1: Constant::new(bits::<SECTOR_TICK_W>(DEFAULT_SECTOR_TICK_WRAP)),
            sector_wake: dff::DFF::new(false),
            transfer_remaining: dff::DFF::new(bits::<10>(0)),
            current_word_position: dff::DFF::new(bits::<8>(0)),
            sector_buffer: dff::DFF::new(buf),
        }
    }

    /// Construct a DiabloDisk with a CUSTOM sector period in cycles.
    /// Intended for tests that want to observe sector-mark behaviour
    /// without running ~20,000 cycles per boundary.  `period_cycles`
    /// must be at least 1 and at most `2^15 = 32768`.
    ///
    /// Real hardware uses [`SECTOR_PERIOD_CYCLES`] (19,608); only use
    /// this constructor in tests.  The standard
    /// [`DiabloDisk::default`] / [`DiabloDisk::with_sector`] /
    /// [`DiabloDisk::with_sector_at_boundary`] constructors all use
    /// the spec-correct period.
    pub fn with_test_period(period_cycles: u32) -> Self {
        debug_assert!(period_cycles >= 1, "sector period must be ≥ 1 cycle");
        debug_assert!(
            period_cycles <= (1u32 << SECTOR_TICK_W),
            "sector period exceeds counter width",
        );
        let wrap_value = (period_cycles - 1) as u128;
        Self {
            sector_tick: dff::DFF::new(bits::<SECTOR_TICK_W>(0)),
            sector_period_minus_1: Constant::new(bits::<SECTOR_TICK_W>(wrap_value)),
            sector_wake: dff::DFF::new(false),
            transfer_remaining: dff::DFF::new(bits::<10>(0)),
            current_word_position: dff::DFF::new(bits::<8>(0)),
            sector_buffer: dff::DFF::new([bits::<16>(0); 256]),
        }
    }

    /// As [`DiabloDisk::with_test_period_and_sector`] but ALSO starts
    /// `sector_tick = period_cycles - 1` so the very first cycle's
    /// wrap-check fires sector_mark immediately.  Matches ContrAlto's
    /// `_sectorEvent = new Event(0, ...)` simulation choice — useful
    /// for lockstep harnesses that want to align task arbitration to
    /// the first cycle.  Steady-state period is unchanged.
    pub fn with_test_period_and_sector_at_boundary(period_cycles: u32, words: &[u16; 256]) -> Self {
        debug_assert!(period_cycles >= 1, "sector period must be ≥ 1 cycle");
        debug_assert!(
            period_cycles <= (1u32 << SECTOR_TICK_W),
            "sector period exceeds counter width",
        );
        let wrap_value = (period_cycles - 1) as u128;
        let mut buf = [bits::<16>(0); 256];
        for (i, &w) in words.iter().enumerate() {
            buf[i] = bits::<16>(w as u128);
        }
        Self {
            sector_tick: dff::DFF::new(bits::<SECTOR_TICK_W>(wrap_value)),
            sector_period_minus_1: Constant::new(bits::<SECTOR_TICK_W>(wrap_value)),
            sector_wake: dff::DFF::new(false),
            transfer_remaining: dff::DFF::new(bits::<10>(0)),
            current_word_position: dff::DFF::new(bits::<8>(0)),
            sector_buffer: dff::DFF::new(buf),
        }
    }

    /// As [`DiabloDisk::with_test_period`] but additionally pre-loads
    /// the sector buffer with `words`.  Tests use this when they need
    /// both a fast sector cadence AND boot-sector content.
    pub fn with_test_period_and_sector(period_cycles: u32, words: &[u16; 256]) -> Self {
        debug_assert!(period_cycles >= 1, "sector period must be ≥ 1 cycle");
        debug_assert!(
            period_cycles <= (1u32 << SECTOR_TICK_W),
            "sector period exceeds counter width",
        );
        let wrap_value = (period_cycles - 1) as u128;
        let mut buf = [bits::<16>(0); 256];
        for (i, &w) in words.iter().enumerate() {
            buf[i] = bits::<16>(w as u128);
        }
        Self {
            sector_tick: dff::DFF::new(bits::<SECTOR_TICK_W>(0)),
            sector_period_minus_1: Constant::new(bits::<SECTOR_TICK_W>(wrap_value)),
            sector_wake: dff::DFF::new(false),
            transfer_remaining: dff::DFF::new(bits::<10>(0)),
            current_word_position: dff::DFF::new(bits::<8>(0)),
            sector_buffer: dff::DFF::new(buf),
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

    // Sector tick: count up to `sector_period_minus_1`, then wrap.
    // Default period is the spec-correct 19,608 cycles; tests can use
    // a shorter period via `DiabloDisk::with_test_period`.
    let next_tick: Bits<SECTOR_TICK_W> = q.sector_tick + bits::<SECTOR_TICK_W>(1);
    let wraps: bool = q.sector_tick == q.sector_period_minus_1;
    d.sector_tick = if wraps {
        bits::<SECTOR_TICK_W>(0)
    } else {
        next_tick
    };

    // Sustained SECTOR-task wakeup per *Alto Hardware Manual* §2.4 +
    // spec §5.5.  Set when sector_tick wraps (sector boundary
    // detected); cleared when current_task=4 issues F1=Block ("the
    // device interface monitors the F1 lines and clears its own
    // wakeup when it sees its task asserting F1=3").
    let block_clears_sector: bool = i.block_task && i.current_task == bits::<4>(4);
    let next_sector_wake: bool = if block_clears_sector {
        false
    } else if wraps {
        true
    } else {
        q.sector_wake
    };
    d.sector_wake = next_sector_wake;
    o.sector_mark = next_sector_wake;

    // Word strobe: sustained while a transfer is in progress, AND not
    // currently being cleared by a Block from current_task=14 (Disk
    // Word).  Real Alto's word_strobe is sustained between the disk
    // controller's per-word strobes; the device deasserts on Block per
    // §5.5.  Phase-3.5 simplification: word_strobe is purely derived
    // from `transfer_remaining > 0` (no separate strobe DFF), but
    // Block clears it for one cycle so the chip-level priority
    // encoder lets a different task win.  Once current_task changes,
    // word_strobe re-asserts (still in transfer), which re-wins
    // arbitration on the next yield — matching real-Alto per-word
    // re-arbitration behavior.
    let block_clears_word: bool = i.block_task && i.current_task == bits::<4>(14);
    o.word_strobe = (q.transfer_remaining != bits::<10>(0)) && !block_clears_word;

    // Combinational read of the addressed word.
    let read_idx: Bits<8> = i.word_addr;
    o.read_data = if i.read_en {
        q.sector_buffer[read_idx]
    } else {
        bits::<16>(0)
    };

    // Apply write to the active sector buffer.
    let mut next_buffer = q.sector_buffer;
    if i.write_en {
        next_buffer[i.word_addr] = i.write_data;
    }
    d.sector_buffer = next_buffer;

    // Transfer countdown.  Decoupled from raw cycle count: only
    // decrements when the engine asserts `word_consumed` (per actual
    // DMA write).  When `transfer_request` arms, set to 256 (overrides
    // the countdown).
    d.transfer_remaining = if i.transfer_request {
        bits::<10>(256)
    } else if i.word_consumed && q.transfer_remaining != bits::<10>(0) {
        q.transfer_remaining - bits::<10>(1)
    } else {
        q.transfer_remaining
    };

    // Current rotational word position: reset to 0 on transfer arm;
    // increment on `word_consumed`; hold otherwise.
    d.current_word_position = if i.transfer_request {
        bits::<8>(0)
    } else if i.word_consumed && q.transfer_remaining != bits::<10>(0) {
        q.current_word_position + bits::<8>(1)
    } else {
        q.current_word_position
    };

    // Expose the current word's data for DMA reads.
    o.current_word_position = q.current_word_position;
    o.current_word_data = q.sector_buffer[q.current_word_position];

    o.ready = true;

    if cr.reset.any() {
        d.sector_tick = bits::<SECTOR_TICK_W>(0);
        d.sector_wake = false;
        d.transfer_remaining = bits::<10>(0);
        d.current_word_position = bits::<8>(0);
        d.sector_buffer = [bits::<16>(0); 256];
        o.sector_mark = false;
        o.word_strobe = false;
        o.read_data = bits::<16>(0);
        o.current_word_position = bits::<8>(0);
        o.current_word_data = bits::<16>(0);
        o.ready = false;
    }

    (o, d)
}
