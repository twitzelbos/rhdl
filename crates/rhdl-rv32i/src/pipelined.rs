//! 5-stage pipelined RV32I core.
//!
//! Per `tier-c-flagship-cores.md` §3.5 Phase 2.  Executes one
//! instruction per cycle in steady state, with full hazard
//! detection (load-use stalls + forwarding) and a 2-cycle branch
//! mispredict penalty (predict-not-taken; squash IF/ID and ID/EX
//! when EX resolves a taken branch).
//!
//! ## Microarchitecture
//!
//! Five stages — Fetch / Decode / Execute / Memory / Writeback —
//! separated by four inter-stage register bundles (`IfId`,
//! `IdEx`, `ExMem`, `MemWb`).  All four bundles plus the PC and
//! the register file are the widget's state.  The kernel
//! implements every stage combinationally each cycle, in reverse
//! order (W → M → E → D → F) so the regfile-write side of
//! Writeback feeds the same-cycle regfile-read side of Decode and
//! the test harness sees results immediately.
//!
//! ## Validation strategy
//!
//! Per the plan, the v0.1 single-cycle [`crate::cpu::Cpu`] is
//! the executable specification.  Tests in `tests/pipelined.rs`
//! run the same program through both cores and compare:
//!
//! - **Sequential ALU programs** — should byte-identically agree
//!   on the final architectural state.
//! - **Programs with hazards (back-to-back ADD using a previous
//!   ADD's result)** — pipelined must produce the same result via
//!   forwarding.
//! - **Load-use programs** — pipelined must produce the same
//!   result via the 1-cycle stall + MEM/WB forwarding.
//! - **Branches** — pipelined must produce the same result via
//!   the 2-cycle squash + redirect.
//!
//! ## What's not in v0.2
//!
//! - **CSRs and traps** — Phase 3 work.  ECALL/EBREAK still set
//!   the `illegal` flag rather than vectoring.
//! - **Memory interface using `RCStream`** — combinational ports
//!   for now per v0.1.  RCStream switch is Phase 2.5+ work.
//! - **JALR target / branch target alignment checks** — RV32I
//!   requires misaligned-target trap on branch/JAL/JALR.  v0.2
//!   silently masks bit 0 to zero (matches v0.1 behaviour).
//!
//! See `tests/pipelined.rs` for runnable examples.

use crate::alu::alu;
use crate::cpu::{branch_taken, load_format};
use crate::decoder::decode;
use crate::hazard::{detect_load_use_stall, forward_select, writes_back};
use crate::isa::{AluSrc, MemOp, Opcode, WritebackSrc};
use crate::pipeline::{ExMem, ForwardSrc, IdEx, IfId, MemWb};
use crate::reg_file::{In as RegIn, RegFile};
use rhdl::prelude::*;
use rhdl_fpga::core::dff;

/// Inputs to the pipelined core.  Same shape as the single-cycle
/// [`crate::cpu::In`] so the same test harness drives both.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct In {
    /// Instruction word fetched at address `pc`.
    pub instr: Bits<32>,
    /// Data-memory read response.
    pub mem_rdata: Bits<32>,
}

/// Outputs from the pipelined core.  Same shape as the single-cycle
/// [`crate::cpu::Out`].
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct Out {
    /// Current program counter (Fetch stage's PC).
    pub pc: Bits<32>,
    /// Data-memory write address (from EX/MEM).
    pub mem_addr: Bits<32>,
    /// Data-memory write data (from EX/MEM).
    pub mem_wdata: Bits<32>,
    /// Data-memory write enable (from EX/MEM).
    pub mem_write: bool,
    /// Data-memory read enable (from EX/MEM).
    pub mem_read: bool,
    /// Memory access width (from EX/MEM).
    pub mem_op: MemOp,
}

/// 5-stage pipelined RV32I CPU widget.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct PipelinedCpu {
    /// Program counter — what Fetch will use next cycle.
    pc: dff::DFF<Bits<32>>,
    /// IF/ID inter-stage register.
    if_id: dff::DFF<IfId>,
    /// ID/EX inter-stage register.
    id_ex: dff::DFF<IdEx>,
    /// EX/MEM inter-stage register.
    ex_mem: dff::DFF<ExMem>,
    /// MEM/WB inter-stage register.
    mem_wb: dff::DFF<MemWb>,
    /// Architectural register file (32×32 bits).
    rf: RegFile,
}

impl Default for PipelinedCpu {
    fn default() -> Self {
        Self {
            pc: dff::DFF::new(bits::<32>(0)),
            if_id: dff::DFF::new(IfId::default()),
            id_ex: dff::DFF::new(IdEx::default()),
            ex_mem: dff::DFF::new(ExMem::default()),
            mem_wb: dff::DFF::new(MemWb::default()),
            rf: RegFile::default(),
        }
    }
}

impl SynchronousIO for PipelinedCpu {
    type I = In;
    type O = Out;
    type Kernel = pipelined_cpu_kernel;
}

/// Apply the forwarding selection to an Execute-stage operand.
#[kernel]
pub fn forward_value(
    sel: ForwardSrc,
    id_ex_val: Bits<32>,
    ex_mem_alu: Bits<32>,
    mem_wb_value: Bits<32>,
) -> Bits<32> {
    match sel {
        ForwardSrc::None  => id_ex_val,
        ForwardSrc::ExMem => ex_mem_alu,
        ForwardSrc::MemWb => mem_wb_value,
    }
}

#[kernel]
/// 5-stage pipelined CPU kernel.
///
/// Stages computed in reverse (W → M → E → D → F) so each stage
/// sees the previous stage's pre-firing register and produces the
/// next-cycle value for its own outgoing register.
///
/// Hazard logic:
/// - **Forwarding**: `forward_select` picks ExMem / MemWb / None
///   for each ALU operand based on the in-flight destination
///   registers.
/// - **Load-use stall**: when ID/EX is a load whose destination
///   matches IF/ID's source registers, freeze PC + IF/ID and
///   replace the next ID/EX with a bubble.
/// - **Branch squash**: when EX resolves a taken branch / JAL /
///   JALR, redirect PC to the target and bubble the next IF/ID
///   and ID/EX.
pub fn pipelined_cpu_kernel(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    let mut o = Out::dont_care();

    // ---- Writeback stage (W) ----------------------------------------
    //
    // Drive the register-file write port from MEM/WB.

    // ---- Memory stage (M) -------------------------------------------
    //
    // Compute MEM/WB-next from EX/MEM and the data-memory read.

    let mem_value: Bits<32> = load_format(q.ex_mem.mem_op, i.mem_rdata);
    let next_mem_wb_writeback: Bits<32> = match q.ex_mem.writeback_src {
        WritebackSrc::None    => bits::<32>(0),
        WritebackSrc::Alu     => q.ex_mem.alu_result,
        WritebackSrc::Mem     => mem_value,
        WritebackSrc::PcPlus4 => q.ex_mem.pc_plus_4,
        // Phase 3 CSR support not yet wired into the pipelined
        // core; for now any Csr writeback is reported as zero.
        // The single-cycle CPU is the reference implementation
        // for CSR semantics; pipelined-CSR support is a follow-up.
        WritebackSrc::Csr     => bits::<32>(0),
    };
    let next_mem_wb_en: bool = q.ex_mem.valid
        && q.ex_mem.writeback_src != WritebackSrc::None
        && q.ex_mem.rd != bits::<5>(0);

    let next_mem_wb = MemWb {
        rd: q.ex_mem.rd,
        writeback_value: next_mem_wb_writeback,
        writeback_en: next_mem_wb_en,
        valid: q.ex_mem.valid,
    };

    // Drive memory ports from EX/MEM (they need to be visible
    // this cycle so the parent's memory model can respond).
    o.mem_addr = q.ex_mem.alu_result;
    o.mem_wdata = q.ex_mem.rs2_val;
    o.mem_write = q.ex_mem.valid && q.ex_mem.mem_write;
    o.mem_read = q.ex_mem.valid && q.ex_mem.mem_read;
    o.mem_op = q.ex_mem.mem_op;

    // ---- Execute stage (E) ------------------------------------------
    //
    // Apply forwarding, run the ALU, decide branches.  Compute
    // EX/MEM-next.

    // Forwarding sources — based on the in-flight EX/MEM and MEM/WB.
    let ex_mem_writes: bool = q.ex_mem.valid && writes_back(q.ex_mem.writeback_src);
    let mem_wb_writes: bool = q.mem_wb.valid && q.mem_wb.writeback_en;

    let fwd_a: ForwardSrc = forward_select(
        q.id_ex.rs1, q.ex_mem.rd, ex_mem_writes,
        q.mem_wb.rd, mem_wb_writes,
    );
    let fwd_b: ForwardSrc = forward_select(
        q.id_ex.rs2, q.ex_mem.rd, ex_mem_writes,
        q.mem_wb.rd, mem_wb_writes,
    );

    let rs1_fwd: Bits<32> = forward_value(
        fwd_a, q.id_ex.rs1_val, q.ex_mem.alu_result, q.mem_wb.writeback_value,
    );
    let rs2_fwd: Bits<32> = forward_value(
        fwd_b, q.id_ex.rs2_val, q.ex_mem.alu_result, q.mem_wb.writeback_value,
    );

    // ALU operand A: PC for AUIPC, otherwise rs1 (forwarded).
    let alu_a: Bits<32> = if q.id_ex.opcode == Opcode::Auipc {
        q.id_ex.pc
    } else {
        rs1_fwd
    };
    // ALU operand B: immediate or rs2 (forwarded).
    let alu_b: Bits<32> = if q.id_ex.alu_src == AluSrc::Imm {
        q.id_ex.imm
    } else {
        rs2_fwd
    };

    let alu_result: Bits<32> = alu(q.id_ex.alu_op, alu_a, alu_b);

    let pc_plus_4_ex: Bits<32> = q.id_ex.pc + bits::<32>(4);

    // Branch / jump resolution.
    let branch_t: bool = branch_taken(q.id_ex.branch_op, rs1_fwd, rs2_fwd);
    let take_branch: bool = q.id_ex.valid && q.id_ex.opcode == Opcode::Branch && branch_t;

    let branch_target: Bits<32> = q.id_ex.pc + q.id_ex.imm;
    let jal_target: Bits<32>    = q.id_ex.pc + q.id_ex.imm;
    let jalr_target: Bits<32>   = (rs1_fwd + q.id_ex.imm) & bits::<32>(0xFFFF_FFFE);

    let take_jal: bool  = q.id_ex.valid && q.id_ex.opcode == Opcode::Jal;
    let take_jalr: bool = q.id_ex.valid && q.id_ex.is_jalr;

    let redirect: bool = take_branch || take_jal || take_jalr;
    let redirect_target: Bits<32> = if take_jalr {
        jalr_target
    } else if take_jal {
        jal_target
    } else {
        branch_target
    };

    let next_ex_mem = ExMem {
        rd: q.id_ex.rd,
        alu_result,
        rs2_val: rs2_fwd,
        mem_op: q.id_ex.mem_op,
        writeback_src: q.id_ex.writeback_src,
        mem_write: q.id_ex.valid && q.id_ex.mem_write,
        mem_read: q.id_ex.valid && q.id_ex.mem_read,
        pc_plus_4: pc_plus_4_ex,
        valid: q.id_ex.valid,
    };

    // ---- Decode stage (D) -------------------------------------------
    //
    // Decode IF/ID's instruction; read source registers via the
    // RegFile sub-widget; compute ID/EX-next.

    let dec = decode(q.if_id.instr);

    // Read register-file values via `q.rf` (driven from `d.rf`
    // below — combinational read ports).
    let id_rs1_val: Bits<32> = q.rf.rdata1;
    let id_rs2_val: Bits<32> = q.rf.rdata2;

    // Load-use stall: based on ID/EX's load destination + the
    // current IF/ID instruction's source registers.
    let stall: bool = q.if_id.valid && detect_load_use_stall(
        q.id_ex.valid && q.id_ex.mem_read,
        q.id_ex.rd,
        dec.rs1,
        dec.rs2,
    );

    let next_id_ex_real = IdEx {
        pc: q.if_id.pc,
        opcode: dec.opcode,
        rd: dec.rd,
        rs1: dec.rs1,
        rs2: dec.rs2,
        imm: dec.imm,
        alu_op: dec.alu_op,
        alu_src: dec.alu_src,
        branch_op: dec.branch_op,
        mem_op: dec.mem_op,
        writeback_src: dec.writeback_src,
        mem_write: dec.mem_write,
        mem_read: dec.mem_read,
        jump: dec.jump,
        is_jalr: dec.is_jalr,
        rs1_val: id_rs1_val,
        rs2_val: id_rs2_val,
        valid: q.if_id.valid,
    };
    let bubble: IdEx = IdEx::default(); // valid = false → behaves as NOP

    // On a stall, ID/EX gets a bubble (load result will be in
    // MEM/WB next cycle and forwarding picks it up).  On a branch
    // squash, ID/EX also gets a bubble (the in-flight Decode is
    // wrong).
    let next_id_ex: IdEx = if stall || redirect {
        bubble
    } else {
        next_id_ex_real
    };

    // ---- Fetch stage (F) --------------------------------------------
    //
    // Latch (PC, instr) into IF/ID-next.  Stall freezes the slot.

    let next_if_id_real = IfId {
        pc: q.pc,
        instr: i.instr,
        valid: true,
    };
    let next_if_id: IfId = if redirect {
        IfId::default() // squash
    } else if stall {
        q.if_id // freeze
    } else {
        next_if_id_real
    };

    // PC update: stall freezes; redirect jumps to target;
    // otherwise advance by 4.
    let next_pc: Bits<32> = if redirect {
        redirect_target
    } else if stall {
        q.pc
    } else {
        q.pc + bits::<32>(4)
    };

    // ---- Drive the register file's input port -----------------------
    //
    // Reads come from the Decode stage (driven by IF/ID's
    // instruction's rs1/rs2).  Writes come from MEM/WB.
    d.rf = RegIn {
        raddr1: dec.rs1,
        raddr2: dec.rs2,
        waddr: q.mem_wb.rd,
        wdata: q.mem_wb.writeback_value,
        wen: mem_wb_writes,
    };

    // ---- Commit pipeline registers ----------------------------------
    d.pc = next_pc;
    d.if_id = next_if_id;
    d.id_ex = next_id_ex;
    d.ex_mem = next_ex_mem;
    d.mem_wb = next_mem_wb;

    o.pc = q.pc;

    if cr.reset.any() {
        d.pc = bits::<32>(0);
        d.if_id = IfId::default();
        d.id_ex = IdEx::default();
        d.ex_mem = ExMem::default();
        d.mem_wb = MemWb::default();
        o.pc = bits::<32>(0);
        o.mem_write = false;
        o.mem_read = false;
    }
    (o, d)
}
