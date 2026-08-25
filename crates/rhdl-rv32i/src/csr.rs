//! M-mode control and status register (CSR) file.
//!
//! Per `tier-c-flagship-cores.md` §3.2 (privileged subset): six
//! read-write CSRs plus two read-only constants.  Sufficient for
//! self-hosted execution and for the riscv-tests harness's pass /
//! fail signaling via ECALL.
//!
//! ## CSRs implemented
//!
//! | Address | Name      | RW | Purpose |
//! |---------|-----------|----|---------|
//! | 0x300   | mstatus   | RW | Machine status register (MIE bit 3, MPIE bit 7) |
//! | 0x301   | misa      | RO | ISA encoding — `0x4000_0100` for RV32I |
//! | 0x304   | mie       | RW | Machine interrupt enable (MSIE 3 / MTIE 7 / MEIE 11) |
//! | 0x305   | mtvec     | RW | Trap vector base address |
//! | 0x340   | mscratch  | RW | Scratch register for trap handlers |
//! | 0x341   | mepc      | RW | Saved PC at trap entry |
//! | 0x342   | mcause    | RW | Trap cause code (bit 31 set for interrupts) |
//! | 0x343   | mtval     | RW | Trap value (badaddr or instruction) |
//! | 0x344   | mip       | RO | Machine interrupt pending — mirrors input port |
//! | 0xF14   | mhartid   | RO | Hart ID — always 0 (single hart) |
//!
//! Writes to read-only CSRs are silently dropped; reads always
//! return the constant value.  Writes to the read-write CSRs commit
//! at the next clock edge.
//!
//! ## Interrupt model
//!
//! `mip` is read-only and mirrors the CPU's `int_pending` input
//! port directly — the platform (test harness, in our case) is
//! responsible for asserting the appropriate bits.  `mie` is
//! software-writable.  An interrupt fires when:
//!
//!   `mstatus.MIE && ((mip & mie) & {bit3, bit7, bit11}) != 0`
//!
//! at any inter-instruction boundary.  Trap entry atomically
//! saves `mstatus.MIE` into `mstatus.MPIE`, clears `mstatus.MIE`,
//! and updates `mepc` / `mcause` / `mtval`.  `MRET` restores
//! `mstatus.MIE` from `mstatus.MPIE` and sets `mstatus.MPIE = 1`
//! per the privileged-ISA spec.
//!
//! Unrecognized CSR addresses return 0 on read and silently drop
//! writes.  The RV32I privileged spec requires unimplemented CSR
//! access to trap as illegal instruction; we simplify to no-op
//! for the addresses our tests don't exercise.  Tracked as a
//! follow-up.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;

// CSR addresses (12-bit).  Constants exported for use by tests
// and the CPU executor.
pub const CSR_MSTATUS: u32 = 0x300;
pub const CSR_MISA: u32 = 0x301;
pub const CSR_MIE: u32 = 0x304;
pub const CSR_MTVEC: u32 = 0x305;
pub const CSR_MSCRATCH: u32 = 0x340;
pub const CSR_MEPC: u32 = 0x341;
pub const CSR_MCAUSE: u32 = 0x342;
pub const CSR_MTVAL: u32 = 0x343;
pub const CSR_MIP: u32 = 0x344;
pub const CSR_MHARTID: u32 = 0xF14;

/// `misa` value reported to the program: bit 8 = "I" extension;
/// bits 31:30 = 01 indicating XLEN = 32.
pub const MISA_VALUE: u32 = (1 << 30) | (1 << 8);

/// `mstatus.MIE` (Machine Interrupt Enable) — bit 3.
pub const MSTATUS_MIE_BIT: u32 = 3;
/// `mstatus.MPIE` (Machine Previous Interrupt Enable) — bit 7.
pub const MSTATUS_MPIE_BIT: u32 = 7;

/// Bit positions inside `mip` / `mie` for the three M-mode
/// interrupt sources (per RISC-V privileged spec).
pub const MIE_MSIE_BIT: u32 = 3; // M-software
pub const MIE_MTIE_BIT: u32 = 7; // M-timer
pub const MIE_MEIE_BIT: u32 = 11; // M-external

/// Combined mask of the three M-mode interrupt-source bits.
/// `mip & mie & MIE_M_MASK` is the set of pending+enabled M-mode
/// interrupts.
pub const MIE_M_MASK: u32 = (1 << MIE_MSIE_BIT) | (1 << MIE_MTIE_BIT) | (1 << MIE_MEIE_BIT);

/// `mcause` interrupt cause codes (with bit 31 set for interrupts).
pub const MCAUSE_M_SOFTWARE: u32 = 0x8000_0003;
pub const MCAUSE_M_TIMER: u32 = 0x8000_0007;
pub const MCAUSE_M_EXTERNAL: u32 = 0x8000_000B;

/// Inputs to the CSR file.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct In {
    /// CSR address to read from.  Read returns combinationally.
    pub raddr: Bits<12>,
    /// CSR address to write.  When `wen` is false, no write occurs.
    pub waddr: Bits<12>,
    /// Data to write to `waddr`.
    pub wdata: Bits<32>,
    /// Write enable.
    pub wen: bool,
    /// Trap-side write port: when `trap_en` is true, mepc/mcause/mtval
    /// are loaded from `trap_pc`/`trap_cause`/`trap_val`, AND
    /// `mstatus.MIE` is saved into `mstatus.MPIE` and then cleared.
    /// This is separate from the CSR-instruction write port so a
    /// trap doesn't have to go through CSRRW.
    pub trap_en: bool,
    pub trap_pc: Bits<32>,
    pub trap_cause: Bits<32>,
    pub trap_val: Bits<32>,
    /// MRET-side: when `mret_en` is true, restore `mstatus.MIE`
    /// from `mstatus.MPIE` and set `mstatus.MPIE = 1` per the
    /// privileged-ISA spec.  Mutually exclusive with `trap_en`
    /// (the executor never asserts both in the same cycle).
    pub mret_en: bool,
    /// External interrupt-pending input.  Mirrors directly into
    /// `mip` (which is therefore read-only from software's POV).
    /// Only bits 3 (MSI), 7 (MTI), and 11 (MEI) are meaningful;
    /// all other bits are ignored.
    pub int_pending: Bits<32>,
}

/// Output from the CSR file.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct Out {
    /// Data read from the CSR at `raddr` (combinational).
    pub rdata: Bits<32>,
    /// Current `mtvec` — exposed directly so the CPU can vector
    /// traps without going through the read port.
    pub mtvec: Bits<32>,
    /// Current `mepc` — exposed directly so the CPU can compute
    /// the MRET target without going through the read port.
    pub mepc: Bits<32>,
    /// Current `mstatus.MIE` — exposed for the CPU's interrupt-
    /// pending computation (avoids round-tripping through the
    /// CSR read port for every cycle).
    pub mstatus_mie: bool,
    /// Pending+enabled M-mode interrupts: `mip & mie & MIE_M_MASK`.
    /// When non-zero AND `mstatus_mie` is set, an interrupt fires.
    pub int_pending_enabled: Bits<32>,
}

/// CSR file widget — eight read-write registers as separate DFFs.
/// `mip` exposed via CSR 0x344 is the OR of two sources: the
/// `int_pending` input (M-timer / M-external bits 7, 11) and the
/// software-writable MSIP register (bit 3).  Per the RISC-V spec,
/// MSIP is software-writable in M-mode; we model the platform side
/// (MTIP / MEIP) as input-driven and MSIP as software-driven.
#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct CsrFile {
    mstatus: dff::DFF<Bits<32>>,
    mie: dff::DFF<Bits<32>>,
    mtvec: dff::DFF<Bits<32>>,
    mscratch: dff::DFF<Bits<32>>,
    mepc: dff::DFF<Bits<32>>,
    mcause: dff::DFF<Bits<32>>,
    mtval: dff::DFF<Bits<32>>,
    /// Software-writable MSIP bit.  When software writes mip via
    /// CSRRW/CSRRS/CSRRC, only bit 3 (MSIP) is captured here; bits
    /// 7 (MTIP) and 11 (MEIP) are read-only and come from input.
    /// The effective `mip` value seen by software is
    /// `(int_pending & ~MSIE_BIT) | (msip << 3)`.
    msip: dff::DFF<bool>,
}

impl SynchronousIO for CsrFile {
    type I = In;
    type O = Out;
    type Kernel = csr_file_kernel;
}

#[kernel]
/// Read the CSR addressed by `addr`.  Read-only CSRs return their
/// hardcoded values; unimplemented addresses return 0.  `mip` is
/// derived from the live `int_pending` input rather than a stored
/// register.
// `mhartid` reads zero because this is a single-hart core, and the
// fallback reads zero because an unimplemented CSR must. Same value,
// different reasons; merging them would delete that distinction.
#[allow(clippy::if_same_then_else)]
pub fn csr_read(addr: Bits<12>, q: Q, mip: Bits<32>) -> Bits<32> {
    let mstatus_a: Bits<12> = bits::<12>(0x300);
    let misa_a: Bits<12> = bits::<12>(0x301);
    let mie_a: Bits<12> = bits::<12>(0x304);
    let mtvec_a: Bits<12> = bits::<12>(0x305);
    let mscratch_a: Bits<12> = bits::<12>(0x340);
    let mepc_a: Bits<12> = bits::<12>(0x341);
    let mcause_a: Bits<12> = bits::<12>(0x342);
    let mtval_a: Bits<12> = bits::<12>(0x343);
    let mip_a: Bits<12> = bits::<12>(0x344);
    let mhartid_a: Bits<12> = bits::<12>(0xF14);
    let misa_v: Bits<32> = bits::<32>(0x4000_0100);

    if addr == mstatus_a {
        q.mstatus
    } else if addr == misa_a {
        misa_v
    } else if addr == mie_a {
        q.mie
    } else if addr == mtvec_a {
        q.mtvec
    } else if addr == mscratch_a {
        q.mscratch
    } else if addr == mepc_a {
        q.mepc
    } else if addr == mcause_a {
        q.mcause
    } else if addr == mtval_a {
        q.mtval
    } else if addr == mip_a {
        mip
    } else if addr == mhartid_a {
        bits::<32>(0)
    } else {
        bits::<32>(0)
    }
}

#[kernel]
/// CSR file kernel.  Combinational read on `raddr`; synchronous
/// write on `waddr` when `wen`; trap port writes mepc/mcause/mtval
/// (and updates mstatus.MIE/MPIE) when `trap_en`; mret port
/// restores mstatus.MIE from MPIE when `mret_en`.  When both `wen`
/// and `trap_en` target the same register, the trap-port wins
/// (matches BSV's "trap-takes-priority" convention).
pub fn csr_file_kernel(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    let mut o = Out::dont_care();

    // mip composition: bits 3, 7, 11 of `int_pending` flow in
    // directly (the platform/harness can drive any of them), AND
    // the software-writable MSIP register OR's into bit 3.  Per
    // spec, MSIP is platform-writable (memory-mapped IPI) AND
    // software-writable (CSR write); both paths set the bit.
    let plat_bits: Bits<32> = i.int_pending & bits::<32>(0x888);
    let msip_bit: Bits<32> = if q.msip {
        bits::<32>(0x8)
    } else {
        bits::<32>(0)
    };
    let mip: Bits<32> = plat_bits | msip_bit;

    o.rdata = csr_read(i.raddr, q, mip);
    o.mtvec = q.mtvec;
    o.mepc = q.mepc;

    // Expose mstatus.MIE and the pending+enabled interrupt mask
    // as direct outputs so the CPU's interrupt-detection path
    // doesn't have to round-trip through the CSR read port.
    let mie_m_mask: Bits<32> = bits::<32>(0x888); // bits 3, 7, 11
    let mie_bit: Bits<32> = bits::<32>(8); // 1 << 3
    o.mstatus_mie = (q.mstatus & mie_bit) != bits::<32>(0);
    // Use the composed mip (platform bits 7/11 + software MSIP bit 3).
    o.int_pending_enabled = mip & q.mie & mie_m_mask;

    let mstatus_a: Bits<12> = bits::<12>(0x300);
    let mie_a: Bits<12> = bits::<12>(0x304);
    let mtvec_a: Bits<12> = bits::<12>(0x305);
    let mscratch_a: Bits<12> = bits::<12>(0x340);
    let mepc_a: Bits<12> = bits::<12>(0x341);
    let mcause_a: Bits<12> = bits::<12>(0x342);
    let mtval_a: Bits<12> = bits::<12>(0x343);
    let mip_a: Bits<12> = bits::<12>(0x344);

    // Default: hold every CSR.
    d.mstatus = q.mstatus;
    d.mie = q.mie;
    d.mtvec = q.mtvec;
    d.mscratch = q.mscratch;
    d.mepc = q.mepc;
    d.mcause = q.mcause;
    d.mtval = q.mtval;
    d.msip = q.msip;

    // CSR-instruction write port.
    if i.wen {
        if i.waddr == mstatus_a {
            d.mstatus = i.wdata;
        } else if i.waddr == mie_a {
            d.mie = i.wdata;
        } else if i.waddr == mtvec_a {
            d.mtvec = i.wdata;
        } else if i.waddr == mscratch_a {
            d.mscratch = i.wdata;
        } else if i.waddr == mepc_a {
            d.mepc = i.wdata;
        } else if i.waddr == mcause_a {
            d.mcause = i.wdata;
        } else if i.waddr == mtval_a {
            d.mtval = i.wdata;
        } else if i.waddr == mip_a {
            // Software write to mip: only bit 3 (MSIP) is captured;
            // bits 7 and 11 (MTIP / MEIP) are read-only platform bits.
            d.msip = (i.wdata & bits::<32>(0x8)) != bits::<32>(0);
        }
        // Read-only CSRs (misa, mhartid) and unrecognized
        // addresses silently drop the write.
    }

    // Trap-port writes — take priority over CSR-instruction writes
    // because a trap occurs at the cycle boundary and the in-flight
    // CSRRW would also be squashed.
    //
    // On trap entry, atomically:
    //   - mepc  ← trap_pc
    //   - mcause ← trap_cause
    //   - mtval ← trap_val
    //   - mstatus.MPIE ← mstatus.MIE  (save current enable)
    //   - mstatus.MIE  ← 0            (disable interrupts in handler)
    if i.trap_en {
        d.mepc = i.trap_pc;
        d.mcause = i.trap_cause;
        d.mtval = i.trap_val;

        // Atomic mstatus update: clear bits 3 and 7 (mask 0xFFFFFF77),
        // then OR in (old MIE shifted to MPIE position).
        let old_mie_bit: Bits<32> = q.mstatus & bits::<32>(0x8); // bit 3
        let old_mie_to_mpie: Bits<32> = old_mie_bit << 4; // bit 3 → bit 7
        let mstatus_cleared: Bits<32> = q.mstatus & bits::<32>(0xFFFF_FF77);
        d.mstatus = mstatus_cleared | old_mie_to_mpie;
    } else if i.mret_en {
        // MRET: restore MIE from MPIE; set MPIE to 1 per spec.
        let old_mpie_bit: Bits<32> = q.mstatus & bits::<32>(0x80); // bit 7
        let old_mpie_to_mie: Bits<32> = old_mpie_bit >> 4; // bit 7 → bit 3
        let mstatus_cleared: Bits<32> = q.mstatus & bits::<32>(0xFFFF_FF77);
        d.mstatus = mstatus_cleared | old_mpie_to_mie | bits::<32>(0x80);
    }

    if cr.reset.any() {
        d.mstatus = bits::<32>(0);
        d.mie = bits::<32>(0);
        d.mtvec = bits::<32>(0);
        d.mscratch = bits::<32>(0);
        d.mepc = bits::<32>(0);
        d.mcause = bits::<32>(0);
        d.mtval = bits::<32>(0);
        d.msip = false;
        // Mirror the reset values onto the live outputs so the
        // CPU sees zeros immediately rather than `dont_care`.
        o.rdata = bits::<32>(0);
        o.mtvec = bits::<32>(0);
        o.mepc = bits::<32>(0);
        o.mstatus_mie = false;
        o.int_pending_enabled = bits::<32>(0);
    }
    (o, d)
}
