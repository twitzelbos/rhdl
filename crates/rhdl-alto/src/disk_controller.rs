//! Alto disk controller — the register-bus interface between the
//! microengine's disk tasks and the Diablo 31 drive.
//!
//! Per the *Alto Hardware Manual* §6, the disk subsystem is
//! controlled via a small set of bus-addressable registers.  The
//! Disk Sector and Disk Word tasks read/write these registers via
//! the microengine's BS / F1 / F2 bus-source / aux-function codes.
//!
//! ## Phase-3 register set (subset)
//!
//! | Register | Width | Purpose |
//! |----------|-------|---------|
//! | KSTAT    | 16    | Status: \[ready, error, transfer_active, ...] |
//! | KDATA    | 16    | Data word for read/write transfer |
//! | KCOM     | 16    | Command: \[op, head, ...] |
//! | KADR     | 16    | \[cylinder, sector, head] address |
//! | KCWA     | 16    | DMA control-word memory address |
//! | KCWD     | 16    | DMA data-word memory address |
//!
//! ## Phase-3 simplifications
//!
//! - KCWA / KCWD are stored but not yet used for actual DMA address
//!   generation (the Disk Word task in Phase-3 just reads/writes a
//!   parent-supplied memory port directly).
//! - KSTAT bits beyond `ready`/`transfer_active`/`error` are
//!   ignored.
//! - No interrupt generation (the rule scheduler observes the disk
//!   wakeups via the Diablo 31 widget's outputs directly).

use rhdl::prelude::*;
use rhdl_fpga::core::dff;

/// Inputs to the disk controller (writes from microcode tasks).
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct CtrlIn {
    /// Address of the register being accessed (Phase-3 uses a small
    /// 3-bit address space — 6 registers).
    pub reg_addr: Bits<3>,
    /// Data to write to the addressed register.
    pub write_data: Bits<16>,
    /// Write enable — when true, `write_data` commits to the
    /// register at `reg_addr`.
    pub write_en: bool,
}

/// Outputs from the disk controller (reads to microcode tasks).
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct CtrlOut {
    /// Combinational read of the register at `reg_addr`.
    pub read_data: Bits<16>,
    /// Live exposure of the address-decoded fields, for the disk
    /// drive widget to consume directly.
    pub kadr_cylinder: Bits<8>,
    pub kadr_head: bool,
    pub kadr_sector: Bits<4>,
    pub kdata_word: Bits<16>,
    pub kcom_op: Bits<3>,
    pub kstat_ready: bool,
    /// Asserted when KCOM bit 15 is set (the "start transfer" bit
    /// in the Phase-3.5 simplified KCOM encoding).  Routed to
    /// DiabloDisk.transfer_request to arm a 256-word transfer.
    pub transfer_request: bool,
}

/// Register addresses (Phase-3 subset; will expand as more disk
/// microcode lands).
pub const REG_KSTAT: u32 = 0;
pub const REG_KDATA: u32 = 1;
pub const REG_KCOM:  u32 = 2;
pub const REG_KADR:  u32 = 3;
pub const REG_KCWA:  u32 = 4;
pub const REG_KCWD:  u32 = 5;

/// The disk controller widget — a small register file plus
/// field-decode for KADR / KCOM.
#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct DiskController {
    kstat: dff::DFF<Bits<16>>,
    kdata: dff::DFF<Bits<16>>,
    kcom:  dff::DFF<Bits<16>>,
    kadr:  dff::DFF<Bits<16>>,
    kcwa:  dff::DFF<Bits<16>>,
    kcwd:  dff::DFF<Bits<16>>,
}

impl SynchronousIO for DiskController {
    type I = CtrlIn;
    type O = CtrlOut;
    type Kernel = disk_controller_kernel;
}

#[kernel]
pub fn disk_controller_kernel(cr: ClockReset, i: CtrlIn, q: Q) -> (CtrlOut, D) {
    let mut d = D::dont_care();
    let mut o = CtrlOut::dont_care();

    // Default: hold every register.
    d.kstat = q.kstat;
    d.kdata = q.kdata;
    d.kcom  = q.kcom;
    d.kadr  = q.kadr;
    d.kcwa  = q.kcwa;
    d.kcwd  = q.kcwd;

    // Write port — single addressed register per cycle.
    let kstat_a: Bits<3> = bits::<3>(0);
    let kdata_a: Bits<3> = bits::<3>(1);
    let kcom_a:  Bits<3> = bits::<3>(2);
    let kadr_a:  Bits<3> = bits::<3>(3);
    let kcwa_a:  Bits<3> = bits::<3>(4);
    let kcwd_a:  Bits<3> = bits::<3>(5);

    if i.write_en {
        if i.reg_addr == kstat_a {
            d.kstat = i.write_data;
        } else if i.reg_addr == kdata_a {
            d.kdata = i.write_data;
        } else if i.reg_addr == kcom_a {
            d.kcom = i.write_data;
        } else if i.reg_addr == kadr_a {
            d.kadr = i.write_data;
        } else if i.reg_addr == kcwa_a {
            d.kcwa = i.write_data;
        } else if i.reg_addr == kcwd_a {
            d.kcwd = i.write_data;
        }
    }

    // Read port — combinational mux on `reg_addr`.
    o.read_data = if i.reg_addr == kstat_a {
        q.kstat
    } else if i.reg_addr == kdata_a {
        q.kdata
    } else if i.reg_addr == kcom_a {
        q.kcom
    } else if i.reg_addr == kadr_a {
        q.kadr
    } else if i.reg_addr == kcwa_a {
        q.kcwa
    } else if i.reg_addr == kcwd_a {
        q.kcwd
    } else {
        bits::<16>(0)
    };

    // Field-decode for the disk drive widget.
    // KADR layout (Phase-3 simplification):
    //   bits[15:8] = cylinder
    //   bit  [7]   = head
    //   bits[3:0]  = sector
    //   (other bits reserved)
    o.kadr_cylinder = ((q.kadr >> 8) & bits::<16>(0xFF)).resize();
    o.kadr_head     = ((q.kadr >> 7) & bits::<16>(1)) != bits::<16>(0);
    o.kadr_sector   = (q.kadr & bits::<16>(0xF)).resize();
    o.kdata_word    = q.kdata;
    o.kcom_op       = (q.kcom & bits::<16>(0x7)).resize();
    o.kstat_ready   = (q.kstat & bits::<16>(1)) != bits::<16>(0);
    // Phase-3.5 simplification: KCOM bit 15 = "start transfer".
    // Real Alto uses different KCOM encoding (cylinder/head/sector
    // bits + a few control flags); we'll re-align when boot trace
    // requires it.
    o.transfer_request = (q.kcom & bits::<16>(0x8000)) != bits::<16>(0);

    if cr.reset.any() {
        d.kstat = bits::<16>(0);
        d.kdata = bits::<16>(0);
        d.kcom  = bits::<16>(0);
        d.kadr  = bits::<16>(0);
        d.kcwa  = bits::<16>(0);
        d.kcwd  = bits::<16>(0);
        o.read_data    = bits::<16>(0);
        o.kadr_cylinder = bits::<8>(0);
        o.kadr_head    = false;
        o.kadr_sector  = bits::<4>(0);
        o.kdata_word   = bits::<16>(0);
        o.kcom_op      = bits::<3>(0);
        o.kstat_ready  = false;
        o.transfer_request = false;
    }

    (o, d)
}
