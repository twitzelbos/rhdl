//! Alto R-register file (32 entries × 16 bits).
//!
//! The Alto has two register banks:
//!
//! - **R-registers**: 32 entries, each 16 bits.  Universal — any
//!   task can read or write them.  This module implements them.
//! - **S-registers**: 256 entries (8 banks × 32 entries), each 16
//!   bits.  Task-specific banking; not in Phase 1.
//!
//! Per the Alto Hardware Manual §2.7, the R-register file has a
//! single read port (selected by the microinstruction's RSEL field)
//! and a single write port (also selected by RSEL when an F1/F2
//! function commands an R-load).  Reads are combinational; writes
//! commit at the cycle edge.
//!
//! A 32-element array of `Bits<16>` is used as the storage; the
//! Synchronous-widget pattern bundles them into one DFF per the
//! §3.1 protocol-PHY pattern (CLAUDE.md), avoiding the 12-tuple
//! ceiling on auto-generated `Q`/`D` types.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;

/// Inputs to the R-register file.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct In {
    /// Read address (5 bits → 32 registers).  Read returns combinationally.
    pub raddr: Bits<5>,
    /// Write address.  When `wen` is false, no write occurs.
    pub waddr: Bits<5>,
    /// Data to write to `waddr`.
    pub wdata: Bits<16>,
    /// Write enable.
    pub wen: bool,
}

/// Output: the data at `raddr` (combinational).
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct Out {
    pub rdata: Bits<16>,
}

/// R-register file widget.  32 entries × 16 bits, packaged as one
/// `dff::DFF<[Bits<16>; 32]>` per the §3.1 protocol-PHY pattern
/// (CLAUDE.md) — one DFF for the whole array sidesteps the 12-tuple
/// ceiling on auto-generated `Q`/`D` types and keeps the regfile
/// trivially composable.
#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct RegFile {
    cells: dff::DFF<[Bits<16>; 32]>,
}

impl SynchronousIO for RegFile {
    type I = In;
    type O = Out;
    type Kernel = regfile_kernel;
}

#[kernel]
/// Combinational read on `raddr`; synchronous write on `waddr`
/// when `wen`.  The Alto has no hardwired-zero register
/// (unlike RV32I's x0); R0 is a normal R/W register.
pub fn regfile_kernel(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    let mut o = Out::dont_care();

    o.rdata = q.cells[i.raddr];

    let mut next = q.cells;
    if i.wen {
        next[i.waddr] = i.wdata;
    }
    d.cells = next;

    if cr.reset.any() {
        d.cells = [bits::<16>(0); 32];
        o.rdata = bits::<16>(0);
    }
    (o, d)
}
