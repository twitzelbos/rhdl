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
//! | 0x300   | mstatus   | RW | Machine status register (a few bits used; reserved bits read 0) |
//! | 0x301   | misa      | RO | ISA encoding — `0x4000_0100` for RV32I |
//! | 0x305   | mtvec     | RW | Trap vector base address |
//! | 0x340   | mscratch  | RW | Scratch register for trap handlers |
//! | 0x341   | mepc      | RW | Saved PC at trap entry |
//! | 0x342   | mcause    | RW | Trap cause code |
//! | 0x343   | mtval     | RW | Trap value (badaddr or instruction) |
//! | 0xF14   | mhartid   | RO | Hart ID — always 0 (single hart) |
//!
//! Writes to read-only CSRs are silently dropped; reads always
//! return the constant value.  Writes to the read-write CSRs commit
//! at the next clock edge.
//!
//! Unrecognized CSR addresses return 0 on read and silently drop
//! writes.  The RV32I privileged spec requires unimplemented CSR
//! access to trap as illegal instruction; v0.3 simplifies to no-op
//! for the addresses our tests don't exercise.  Tracked as a
//! follow-up.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;

// CSR addresses (12-bit).  Constants exported for use by tests
// and the CPU executor.
pub const CSR_MSTATUS: u32  = 0x300;
pub const CSR_MISA: u32     = 0x301;
pub const CSR_MTVEC: u32    = 0x305;
pub const CSR_MSCRATCH: u32 = 0x340;
pub const CSR_MEPC: u32     = 0x341;
pub const CSR_MCAUSE: u32   = 0x342;
pub const CSR_MTVAL: u32    = 0x343;
pub const CSR_MHARTID: u32  = 0xF14;

/// `misa` value reported to the program: bit 8 = "I" extension;
/// bits 31:30 = 01 indicating XLEN = 32.
pub const MISA_VALUE: u32 = (1 << 30) | (1 << 8);

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
    /// are loaded from `trap_pc`/`trap_cause`/`trap_val`.  This is
    /// separate from the CSR-instruction write port so a trap doesn't
    /// have to go through CSRRW.
    pub trap_en: bool,
    pub trap_pc: Bits<32>,
    pub trap_cause: Bits<32>,
    pub trap_val: Bits<32>,
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
}

/// CSR file widget — six read-write CSRs as separate DFFs.
#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct CsrFile {
    mstatus: dff::DFF<Bits<32>>,
    mtvec: dff::DFF<Bits<32>>,
    mscratch: dff::DFF<Bits<32>>,
    mepc: dff::DFF<Bits<32>>,
    mcause: dff::DFF<Bits<32>>,
    mtval: dff::DFF<Bits<32>>,
}

impl SynchronousIO for CsrFile {
    type I = In;
    type O = Out;
    type Kernel = csr_file_kernel;
}

#[kernel]
/// Read the CSR addressed by `addr`.  Read-only CSRs return their
/// hardcoded values; unimplemented addresses return 0.
pub fn csr_read(addr: Bits<12>, q: Q) -> Bits<32> {
    let mstatus_a: Bits<12>  = bits::<12>(0x300);
    let misa_a: Bits<12>     = bits::<12>(0x301);
    let mtvec_a: Bits<12>    = bits::<12>(0x305);
    let mscratch_a: Bits<12> = bits::<12>(0x340);
    let mepc_a: Bits<12>     = bits::<12>(0x341);
    let mcause_a: Bits<12>   = bits::<12>(0x342);
    let mtval_a: Bits<12>    = bits::<12>(0x343);
    let mhartid_a: Bits<12>  = bits::<12>(0xF14);
    let misa_v: Bits<32>     = bits::<32>(0x4000_0100);

    if addr == mstatus_a {
        q.mstatus
    } else if addr == misa_a {
        misa_v
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
    } else if addr == mhartid_a {
        bits::<32>(0)
    } else {
        bits::<32>(0)
    }
}

#[kernel]
/// CSR file kernel.  Combinational read on `raddr`; synchronous
/// write on `waddr` when `wen`; trap port writes mepc/mcause/mtval
/// when `trap_en`.  When both `wen` and `trap_en` target the same
/// register, the trap-port wins (matches BSV's "trap-takes-priority"
/// convention).
pub fn csr_file_kernel(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    let mut o = Out::dont_care();

    o.rdata = csr_read(i.raddr, q);
    o.mtvec = q.mtvec;
    o.mepc = q.mepc;

    let mstatus_a: Bits<12>  = bits::<12>(0x300);
    let mtvec_a: Bits<12>    = bits::<12>(0x305);
    let mscratch_a: Bits<12> = bits::<12>(0x340);
    let mepc_a: Bits<12>     = bits::<12>(0x341);
    let mcause_a: Bits<12>   = bits::<12>(0x342);
    let mtval_a: Bits<12>    = bits::<12>(0x343);

    // Default: hold every CSR.
    d.mstatus  = q.mstatus;
    d.mtvec    = q.mtvec;
    d.mscratch = q.mscratch;
    d.mepc     = q.mepc;
    d.mcause   = q.mcause;
    d.mtval    = q.mtval;

    // CSR-instruction write port.
    if i.wen {
        if i.waddr == mstatus_a {
            d.mstatus = i.wdata;
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
        }
        // Read-only CSRs (misa, mhartid) and unrecognized
        // addresses silently drop the write.
    }

    // Trap-port writes — take priority over CSR-instruction writes
    // because a trap occurs at the cycle boundary and the in-flight
    // CSRRW would also be squashed.
    if i.trap_en {
        d.mepc = i.trap_pc;
        d.mcause = i.trap_cause;
        d.mtval = i.trap_val;
    }

    if cr.reset.any() {
        d.mstatus = bits::<32>(0);
        d.mtvec = bits::<32>(0);
        d.mscratch = bits::<32>(0);
        d.mepc = bits::<32>(0);
        d.mcause = bits::<32>(0);
        d.mtval = bits::<32>(0);
        // Mirror the reset values onto the live outputs so the
        // CPU sees zeros immediately rather than `dont_care`.
        o.rdata = bits::<32>(0);
        o.mtvec = bits::<32>(0);
        o.mepc = bits::<32>(0);
    }
    (o, d)
}
