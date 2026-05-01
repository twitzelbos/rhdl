//! Alto ALU — 16 functions over 16-bit operands.
//!
//! The Alto ALU is a 4-bit-controlled combinational unit producing
//! a 16-bit result, a carry-out, and a "shift" output (used by some
//! F2 codes for conditional branching).  Per the *Alto Hardware
//! Manual* §2.5.
//!
//! ## Inputs
//! - `bus` — the BUS operand (16 bits).  Source selected by the
//!   microinstruction's BS field.
//! - `t`   — the T-register operand (16 bits).
//! - `func` — the [`AluFunction`] selected by the microinstruction's
//!   ALUF field.
//! - `skip` — the SKIP signal (used only by `BUS + SKIP`; commonly
//!   driven by carry-out of the previous ALU op).
//!
//! ## Outputs
//! - `result` (16 bits)
//! - `carry`  — bit 17 of the unsigned-extended sum/diff (used by
//!   F2 = AluCarryToNext for conditional branching).

use crate::isa::AluFunction;
use rhdl::prelude::*;

/// Result bundle from one Alto ALU evaluation.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub struct AluOut {
    /// The 16-bit ALU result.
    pub result: Bits<16>,
    /// Carry-out of the ALU (bit 16 of the extended computation).
    /// Only meaningful for arithmetic functions; defined to 0 for
    /// pure-logical functions per the manual.
    pub carry: bool,
}

#[kernel]
/// Alto ALU as a pure combinational kernel.
///
/// The 16 functions are encoded by [`AluFunction`].  Reserved
/// codes 14 and 15 are treated as `BUS` (per "undefined" semantics
/// in the manual — implementations are free to choose; we choose
/// the most-common-base value).
pub fn alu(func: AluFunction, bus: Bits<16>, t: Bits<16>, skip: bool) -> AluOut {
    // Extend operands to 17 bits for carry-out computation.
    let bus17: Bits<17> = bus.resize();
    let t17: Bits<17> = t.resize();
    let one17: Bits<17> = bits::<17>(1);

    let sum: Bits<17> = bus17 + t17;
    let sum_p1: Bits<17> = (bus17 + t17) + one17;
    let bus_p1: Bits<17> = bus17 + one17;
    let bus_m1: Bits<17> = bus17 - one17;
    let diff: Bits<17> = bus17 - t17;
    let diff_m1: Bits<17> = (bus17 - t17) - one17;

    let result17: Bits<17> = match func {
        AluFunction::Bus              => bus17,
        AluFunction::T                => t17,
        AluFunction::BusOrT           => bus17 | t17,
        AluFunction::BusAndT          => bus17 & t17,
        AluFunction::BusXorT          => bus17 ^ t17,
        AluFunction::BusPlusOne       => bus_p1,
        AluFunction::BusMinusOne      => bus_m1,
        AluFunction::BusPlusT         => sum,
        AluFunction::BusMinusT        => diff,
        AluFunction::BusMinusTMinusOne => diff_m1,
        AluFunction::BusPlusTPlusOne  => sum_p1,
        AluFunction::BusPlusSkip      => if skip { bus_p1 } else { bus17 },
        AluFunction::BusAndTAlt       => bus17 & t17,
        AluFunction::BusAndNotT       => bus17 & !t17,
        AluFunction::Undef14          => bus17,
        AluFunction::Undef15          => bus17,
    };

    AluOut {
        result: result17.resize(),
        carry: (result17 & bits::<17>(0x10000)) != bits::<17>(0),
    }
}
