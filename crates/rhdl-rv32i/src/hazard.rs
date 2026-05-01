//! Hazard detection and forwarding-control kernels for the
//! 5-stage pipeline.
//!
//! This module is the schedule-correctness brain of the pipeline.
//! Three responsibilities:
//!
//! 1. **Forwarding selection** — for each Execute-stage source
//!    operand (rs1 and rs2), decide whether to bypass the value
//!    from EX/MEM, from MEM/WB, or take the value out of ID/EX.
//!    Forwarding from EX/MEM beats MEM/WB when both could supply
//!    the same source (newest value wins).
//!
//! 2. **Load-use stall detection** — if the ID/EX stage is a load
//!    and the IF/ID stage is about to consume that load's result
//!    in Execute (i.e. one of its source registers is the load's
//!    destination), insert a bubble into ID/EX so the load result
//!    is available via MEM/WB forwarding next cycle.
//!
//! 3. **Branch squash** — when the Execute stage decides a branch
//!    is taken (or any unconditional jump fires), the two
//!    instructions already fetched into IF/ID and ID/EX must be
//!    squashed (replaced with bubbles).  Combined with the
//!    re-direction of the PC to the branch target, this gives a
//!    2-cycle branch-mispredict penalty — the textbook 5-stage
//!    branch handling.
//!
//! All three are pure combinational kernels.  The pipeline widget
//! invokes them every cycle; their outputs gate the inter-stage
//! register updates.

use crate::isa::WritebackSrc;
use crate::pipeline::ForwardSrc;
use rhdl::prelude::*;

/// Decide the forwarding source for one Execute-stage operand.
///
/// Inputs:
/// - `rs`: the operand's source-register number (the ID/EX rs1 or rs2).
/// - `ex_mem_rd`, `ex_mem_writes`: the EX/MEM stage's destination
///   register and whether it's about to write something back.
/// - `mem_wb_rd`, `mem_wb_writes`: same for the MEM/WB stage.
///
/// Returns the forwarding source the Execute stage should use for
/// this operand.
///
/// **Priority:** EX/MEM forwarding beats MEM/WB.  If the same
/// register is being written by both stages, EX/MEM has the newer
/// value and wins.  Forwarding does not apply to register x0
/// (it's hardwired zero — never forwarded).
#[kernel]
pub fn forward_select(
    rs: Bits<5>,
    ex_mem_rd: Bits<5>,
    ex_mem_writes: bool,
    mem_wb_rd: Bits<5>,
    mem_wb_writes: bool,
) -> ForwardSrc {
    let zero: Bits<5> = bits::<5>(0);
    if ex_mem_writes && ex_mem_rd != zero && ex_mem_rd == rs {
        ForwardSrc::ExMem
    } else if mem_wb_writes && mem_wb_rd != zero && mem_wb_rd == rs {
        ForwardSrc::MemWb
    } else {
        ForwardSrc::None
    }
}

/// Detect a load-use hazard.
///
/// Returns `true` if the instruction in ID/EX is a load AND the
/// instruction currently in IF/ID (about to enter Decode) consumes
/// the load's destination register.  When true, the pipeline
/// stalls one cycle: PC and IF/ID freeze; ID/EX is replaced with
/// a bubble.  Next cycle the load's result is in MEM/WB and the
/// usual forwarding path picks it up.
///
/// `if_id_rs1` and `if_id_rs2` are the source-register numbers the
/// IF/ID instruction would use — extracted from the instruction
/// word at the standard rs1/rs2 bit positions.
#[kernel]
pub fn detect_load_use_stall(
    id_ex_mem_read: bool,
    id_ex_rd: Bits<5>,
    if_id_rs1: Bits<5>,
    if_id_rs2: Bits<5>,
) -> bool {
    let zero: Bits<5> = bits::<5>(0);
    id_ex_mem_read && id_ex_rd != zero && (id_ex_rd == if_id_rs1 || id_ex_rd == if_id_rs2)
}

/// True iff the ID/EX or EX/MEM stage's writeback selector implies
/// the instruction will write something back.  Used by
/// [`forward_select`] to gate whether forwarding from that stage
/// is meaningful.
#[kernel]
pub fn writes_back(src: WritebackSrc) -> bool {
    match src {
        WritebackSrc::None => false,
        WritebackSrc::Alu => true,
        WritebackSrc::Mem => true,
        WritebackSrc::PcPlus4 => true,
        WritebackSrc::Csr => true,
    }
}
