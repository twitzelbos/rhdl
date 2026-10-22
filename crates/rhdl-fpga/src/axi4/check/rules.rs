//! The fault vocabulary, and the rules both monitors share.
//!
//! The write monitor and the read monitor check the *same* address-phase
//! rules — a 4 KiB crossing, a `Wrap` length, a size-aligned `Wrap`
//! start, an `AxSIZE` within the bus — because AW and AR carry the same
//! [`AxiAddress`] and the spec imposes the same constraints on both.
//!
//! Those checks therefore live here once, in [`address_fault`], rather
//! than twice. Two copies would be two places to fix a misreading, and
//! a drift between them would be invisible: each monitor's tests would
//! still pass. Both monitors also share one [`Fault`] enum, so a fault
//! name means the same thing whichever channel reported it.

use super::super::types::{
    AxiAddress, BurstKind, bytes_per_beat, crosses_4k, size_fits_bus, wrap_len_is_legal,
};
use rhdl::prelude::*;

/// A protocol rule a monitor found broken.
///
/// Reported sticky-first: a monitor latches the earliest fault and holds
/// it, because the first divergence is the one with a diagnosable cause.
/// Later faults are usually its consequences, and the running `faults`
/// count says how many there were.
///
/// Shared between the read and write monitors. The address-phase
/// variants are not split per channel — a `WrapUnaligned` from the read
/// monitor is about `ARADDR` and one from the write monitor is about
/// `AWADDR`, and which widget reported it is already known to whoever
/// is looking.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub enum Fault {
    /// No rule broken so far.
    #[default]
    None,
    /// `AWVALID` fell before `AWREADY` rose.
    AwValidDropped,
    /// The AW payload changed while the beat was offered and unaccepted.
    AwPayloadChanged,
    /// `WVALID` fell before `WREADY` rose.
    WValidDropped,
    /// The W payload changed while the beat was offered and unaccepted.
    WPayloadChanged,
    /// `BVALID` fell before `BREADY` rose.
    BValidDropped,
    /// The B payload changed while the beat was offered and unaccepted.
    BPayloadChanged,
    /// `ARVALID` fell before `ARREADY` rose.
    ArValidDropped,
    /// The AR payload changed while the beat was offered and unaccepted.
    ArPayloadChanged,
    /// `RVALID` fell before `RREADY` rose.
    RValidDropped,
    /// The R payload changed while the beat was offered and unaccepted.
    RPayloadChanged,
    /// `WLAST` on a beat before the burst's last.
    WLastEarly,
    /// The burst's last beat arrived without `WLAST`.
    WLastMissing,
    /// `RLAST` on a beat before the burst's last.
    RLastEarly,
    /// The burst's last read beat arrived without `RLAST`.
    RLastMissing,
    /// A write response with no transaction outstanding.
    BrespUnexpected,
    /// A read beat with no transaction outstanding.
    RBeatUnexpected,
    /// A read beat whose `RID` is not the outstanding transaction's.
    RIdMismatch,
    /// An `Incr` burst crossing a 4 KiB boundary.
    IncrCrosses4K,
    /// A `Wrap` burst of other than 2, 4, 8 or 16 beats.
    WrapBadLength,
    /// A `Wrap` burst starting off a size boundary.
    WrapUnaligned,
    /// `AxSIZE` wider than the data bus.
    SizeExceedsBus,
    /// `WSTRB` asserting a lane outside the transfer's selected bytes.
    StrobeOutsideLanes,
}

impl Fault {
    /// Every fault a monitor can report.
    ///
    /// Exists so a test can require every one of them be reachable from
    /// some generated transcript. A check a monitor performs that no
    /// traffic ever triggers is a check that has never run, and two of
    /// them shipped here before a mutation run found them — see the
    /// `check` module docs.
    pub fn all() -> Vec<Fault> {
        vec![
            Fault::None,
            Fault::AwValidDropped,
            Fault::AwPayloadChanged,
            Fault::WValidDropped,
            Fault::WPayloadChanged,
            Fault::BValidDropped,
            Fault::BPayloadChanged,
            Fault::ArValidDropped,
            Fault::ArPayloadChanged,
            Fault::RValidDropped,
            Fault::RPayloadChanged,
            Fault::WLastEarly,
            Fault::WLastMissing,
            Fault::RLastEarly,
            Fault::RLastMissing,
            Fault::BrespUnexpected,
            Fault::RBeatUnexpected,
            Fault::RIdMismatch,
            Fault::IncrCrosses4K,
            Fault::WrapBadLength,
            Fault::WrapUnaligned,
            Fault::SizeExceedsBus,
            Fault::StrobeOutsideLanes,
        ]
    }
}

#[kernel]
/// Latch `new` if `cond` and nothing has been latched yet.
///
/// Sticky-first in one place, so that each monitor's long chain of
/// checks reads as a list of rules rather than a nest of conditionals.
pub fn first_fault(current: Fault, cond: bool, new: Fault) -> Fault {
    match current {
        Fault::None => {
            if cond {
                new
            } else {
                current
            }
        }
        _ => current,
    }
}

#[kernel]
/// Is this burst a `Wrap`?
pub fn is_wrap(b: BurstKind) -> bool {
    match b {
        BurstKind::Wrap => true,
        _ => false,
    }
}

#[kernel]
/// The address-phase rules, which AW and AR share.
///
/// Returns the first rule the address phase breaks, or [`Fault::None`].
/// `bus_bytes` is the data bus width in bytes.
pub fn address_fault<const A: usize, const ID: usize>(
    a: AxiAddress<A, ID>,
    bus_bytes: Bits<9>,
) -> Fault
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    let wrap = is_wrap(a.burst);
    // A power-of-two transfer size no greater than 128 bytes, so the
    // alignment test is an exact mask rather than a division.
    let bytes = bytes_per_beat(a.size).resize::<A>();
    let unaligned = (a.addr & (bytes - bits::<A>(1))) != bits::<A>(0);

    let mut f = Fault::None;
    f = first_fault(f, !size_fits_bus(a.size, bus_bytes), Fault::SizeExceedsBus);
    f = first_fault(
        f,
        crosses_4k::<A>(a.addr, a.size, a.len, a.burst),
        Fault::IncrCrosses4K,
    );
    f = first_fault(f, wrap && !wrap_len_is_legal(a.len), Fault::WrapBadLength);
    f = first_fault(f, wrap && unaligned, Fault::WrapUnaligned);
    f
}
