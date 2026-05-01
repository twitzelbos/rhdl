//! 32×32-bit RV32I register file.
//!
//! Synchronous write, asynchronous (combinational) read.  Register
//! x0 is hardwired to zero — writes to x0 are silently dropped,
//! reads always return 0.  Two read ports (rs1, rs2) and one write
//! port (rd).
//!
//! Implemented as a single `dff::DFF<[Bits<32>; 32]>` — one DFF
//! whose state is a 32-element array of 32-bit values.  This is
//! the bundled-state pattern from CLAUDE.md §3.1; expressing the
//! 32 architectural registers as 32 separate DFF fields would hit
//! the auto-derived `Q`/`D` 12-element tuple ceiling, and packing
//! them into a `Bits<1024>` exceeds RHDL's per-`BitWidth`
//! coverage (which currently tops out around 128 bits).

use rhdl::prelude::*;
use rhdl_fpga::core::dff;

/// Inputs to the register file.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct In {
    /// Read-port 1 address (rs1).
    pub raddr1: Bits<5>,
    /// Read-port 2 address (rs2).
    pub raddr2: Bits<5>,
    /// Write-port address (rd).  When equal to 0, the write is
    /// silently dropped (x0 is hardwired to zero per the spec).
    pub waddr: Bits<5>,
    /// Write-port data.
    pub wdata: Bits<32>,
    /// Write-port enable — when false, no register is written
    /// regardless of `waddr` / `wdata`.
    pub wen: bool,
}

/// Outputs from the register file.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct Out {
    /// Read-port 1 data (combinational from `raddr1`).
    pub rdata1: Bits<32>,
    /// Read-port 2 data (combinational from `raddr2`).
    pub rdata2: Bits<32>,
}

/// 32×32-bit register file widget.
///
/// State is bundled into a single `[Bits<32>; 32]` array per the
/// CLAUDE.md §3.1 "single-FSM-DFF + bundled-state-DFF" pattern.
#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct RegFile {
    /// All 32 registers as one DFF.  Index 0 (x0) is read as zero
    /// regardless of the underlying value; writes to it are
    /// dropped — we don't bother force-clearing because the kernel
    /// guards reads/writes on `addr == 0`.
    regs: dff::DFF<[Bits<32>; 32]>,
}

impl SynchronousIO for RegFile {
    type I = In;
    type O = Out;
    type Kernel = reg_file_kernel;
}

#[kernel]
/// Register-file kernel.  Asynchronous reads from the current
/// (pre-firing) `regs` snapshot; synchronous write commits at the
/// next clock edge.  x0 is hardwired to zero — reads always
/// return zero, writes are silently dropped.
pub fn reg_file_kernel(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    let mut o = Out::dont_care();

    // Read ports — combinational.  x0 always reads as zero.
    o.rdata1 = if i.raddr1 == bits::<5>(0) {
        bits::<32>(0)
    } else {
        q.regs[i.raddr1]
    };
    o.rdata2 = if i.raddr2 == bits::<5>(0) {
        bits::<32>(0)
    } else {
        q.regs[i.raddr2]
    };

    // Write port — copy the current array, splice in the new
    // value if write-enabled and the address isn't x0.
    let mut next: [Bits<32>; 32] = q.regs;
    if i.wen && i.waddr != bits::<5>(0) {
        next[i.waddr] = i.wdata;
    }
    d.regs = next;

    if cr.reset.any() {
        d.regs = [bits::<32>(0); 32];
        o.rdata1 = bits::<32>(0);
        o.rdata2 = bits::<32>(0);
    }
    (o, d)
}
