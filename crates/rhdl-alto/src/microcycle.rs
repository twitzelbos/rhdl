//! Shared per-cycle microinstruction execution.
//!
//! Factored out of [`crate::microengine`] so the single-task version
//! and the 16-task [`crate::task_system`] both share the same
//! semantics — every Alto microcycle does the same BUS-fetch /
//! ALU / T-load / L-load / next-MPC computation regardless of who
//! is running.  The only thing that differs across tasks is *which*
//! task's MPC is updated.
//!
//! This kernel is pure-combinational: it takes the engine's current
//! state plus the running microinstruction and returns the
//! candidate next state.  The caller (microengine or task system)
//! is responsible for committing the result to its DFFs.

use crate::alu::{alu, AluOut};
use crate::isa::{BusSource, F1Function, F2Function, Microinstruction};
use rhdl::prelude::*;

/// Result of one microcycle's combinational evaluation.
#[derive(PartialEq, Eq, Debug, Digital, Clone, Copy, Default)]
pub struct CycleResult {
    /// The 16-bit BUS this cycle (visible for trace).
    pub bus: Bits<16>,
    /// ALU output (result + carry).
    pub aout: AluOut,
    /// Candidate next-cycle T value (commit if T_LOAD).
    pub next_t: Bits<16>,
    /// Candidate next-cycle L value (commit unconditionally —
    /// the kernel folds L_LOAD into this).
    pub next_l: Bits<16>,
    /// Whether the microinstruction wants to write R (BS = LoadR).
    pub r_wen: bool,
    /// Value to write into R[rsel] if r_wen is true.
    pub r_wdata: Bits<16>,
    /// Next MPC (combining NEXT field with F2 modifications).
    pub next_mpc: Bits<10>,
}

#[kernel]
/// Compute the per-cycle update for the Alto microengine.
///
/// Inputs:
/// - `mi` — decoded microinstruction
/// - `t`, `l` — current T and L
/// - `r_read` — the register-file read at index `mi.rsel`
///
/// Outputs: a [`CycleResult`] with all the candidate next-state
/// values; the caller commits them to its DFFs.
pub fn compute_cycle(
    mi: Microinstruction,
    t: Bits<16>,
    l: Bits<16>,
    r_read: Bits<16>,
) -> CycleResult {
    // ---- BUS source ------------------------------------------------
    let bus: Bits<16> = match mi.bs {
        BusSource::ReadR => r_read,
        _                => bits::<16>(0),
    };

    // ---- ALU -------------------------------------------------------
    // SKIP placeholder: previous-L MSB.
    let skip: bool = (l & bits::<16>(0x8000)) != bits::<16>(0);
    let aout: AluOut = alu(mi.aluf, bus, t, skip);

    // ---- T and L latches ------------------------------------------
    let next_t: Bits<16> = if mi.t_load { bus } else { t };
    let l_loaded: Bits<16> = if mi.l_load { aout.result } else { l };
    let l_after_f1: Bits<16> = match mi.f1 {
        F1Function::LeftShift1  => l_loaded << 1,
        F1Function::RightShift1 => l_loaded >> 1,
        F1Function::LeftCycle8  => (l_loaded << 8) | (l_loaded >> 8),
        _                       => l_loaded,
    };

    // ---- Next MPC ------------------------------------------------
    let mut next_addr: Bits<10> = mi.next;
    let mut bit0: Bits<10> = next_addr & bits::<10>(0x1);
    bit0 = match mi.f2 {
        F2Function::BusEqZero          => if bus == bits::<16>(0)             { bit0 | bits::<10>(0x1) } else { bit0 },
        F2Function::ShiftLessThanZero  => if (l_after_f1 & bits::<16>(0x8000)) == bits::<16>(0) { bit0 | bits::<10>(0x1) } else { bit0 },
        F2Function::ShiftEqZero        => if l_after_f1 == bits::<16>(0)      { bit0 | bits::<10>(0x1) } else { bit0 },
        F2Function::AluCarryToNext     => if aout.carry                       { bit0 | bits::<10>(0x1) } else { bit0 },
        _                              => bit0,
    };
    let next_addr_or_bus: Bits<10> = match mi.f2 {
        F2Function::BusToNext => next_addr | (bus.resize() & bits::<10>(0x3FF)),
        _                     => next_addr,
    };
    next_addr = (next_addr_or_bus & bits::<10>(0x3FE)) | bit0;

    CycleResult {
        bus,
        aout,
        next_t,
        next_l: l_after_f1,
        r_wen: mi.bs == BusSource::LoadR,
        r_wdata: aout.result,
        next_mpc: next_addr,
    }
}
