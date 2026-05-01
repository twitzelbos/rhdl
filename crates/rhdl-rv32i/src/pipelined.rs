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
use crate::csr::{CsrFile, In as CsrIn};
use crate::decoder::decode;
use crate::hazard::{detect_load_use_stall, forward_select, writes_back};
use crate::isa::{AluSrc, CsrOp, MemOp, Opcode, SystemOp, WritebackSrc};
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
    /// M-mode CSR file (Phase 3 pipelined support).
    csrs: CsrFile,
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
            csrs: CsrFile::default(),
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
        // For CSR instructions, the Execute stage stashed the
        // pre-modify CSR value in `alu_result` so the writeback
        // path for `rd ← old_csr` is the same shape as for
        // ordinary ALU ops.
        WritebackSrc::Csr     => q.ex_mem.alu_result,
    };
    let next_mem_wb_en: bool = q.ex_mem.valid
        && q.ex_mem.writeback_src != WritebackSrc::None
        && q.ex_mem.rd != bits::<5>(0);

    let next_mem_wb = MemWb {
        rd: q.ex_mem.rd,
        writeback_value: next_mem_wb_writeback,
        writeback_en: next_mem_wb_en,
        // CSR write info propagates verbatim from EX/MEM.  The
        // Writeback stage drives the CSR file's write port from
        // these fields; only commits when csr_writes && valid.
        csr_addr: q.ex_mem.csr_addr,
        csr_new_value: q.ex_mem.csr_new_value,
        csr_writes: q.ex_mem.csr_writes,
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

    // CSR access in Execute stage: the read port is driven this
    // cycle by `d.csrs.raddr = q.id_ex.csr_addr` (below); the
    // pre-modify value comes back via `q.csrs.rdata`.  Compute
    // the new value here and forward it to MEM/WB for commit.
    //
    // For CSR-read-into-rd (writeback_src == Csr), `alu_result`
    // is repurposed to carry the pre-modify CSR value through the
    // pipeline.  This works because the same EX/MEM slot can't
    // hold both an ALU result and a CSR result (writeback_src
    // distinguishes), and the Memory stage re-reads writeback_src
    // to mux into MEM/WB.
    let csr_rdata: Bits<32> = q.csrs.rdata;
    let csr_uimm: Bits<32> = q.id_ex.rs1.resize();
    let csr_src: Bits<32> = match q.id_ex.csr_op {
        CsrOp::ReadWriteImm => csr_uimm,
        CsrOp::ReadSetImm   => csr_uimm,
        CsrOp::ReadClearImm => csr_uimm,
        _                   => rs1_fwd,
    };
    let csr_new_value: Bits<32> = match q.id_ex.csr_op {
        CsrOp::ReadWrite     => csr_src,
        CsrOp::ReadWriteImm  => csr_src,
        CsrOp::ReadSet       => csr_rdata | csr_src,
        CsrOp::ReadSetImm    => csr_rdata | csr_src,
        CsrOp::ReadClear     => csr_rdata & !csr_src,
        CsrOp::ReadClearImm  => csr_rdata & !csr_src,
        CsrOp::None          => csr_rdata,
    };
    let csr_writes_ex: bool = match q.id_ex.csr_op {
        CsrOp::None         => false,
        CsrOp::ReadWrite    => true,
        CsrOp::ReadWriteImm => true,
        CsrOp::ReadSet      => q.id_ex.rs1 != bits::<5>(0),
        CsrOp::ReadSetImm   => q.id_ex.rs1 != bits::<5>(0),
        CsrOp::ReadClear    => q.id_ex.rs1 != bits::<5>(0),
        CsrOp::ReadClearImm => q.id_ex.rs1 != bits::<5>(0),
    };

    // Branch / jump resolution.
    let branch_t: bool = branch_taken(q.id_ex.branch_op, rs1_fwd, rs2_fwd);
    let take_branch_raw: bool = q.id_ex.valid && q.id_ex.opcode == Opcode::Branch && branch_t;

    let branch_target: Bits<32> = q.id_ex.pc + q.id_ex.imm;
    let jal_target: Bits<32>    = q.id_ex.pc + q.id_ex.imm;
    let jalr_target: Bits<32>   = (rs1_fwd + q.id_ex.imm) & bits::<32>(0xFFFF_FFFE);

    let take_jal_raw: bool  = q.id_ex.valid && q.id_ex.opcode == Opcode::Jal;
    let take_jalr_raw: bool = q.id_ex.valid && q.id_ex.is_jalr;

    // Misaligned-target trap detection: any branch/JAL/JALR whose
    // target's low 2 bits are nonzero is rejected with mcause = 0
    // (Instruction address misaligned) and mtval = the misaligned
    // target.  The redirect itself is suppressed — the redirect
    // becomes the trap-vector redirect instead.
    let prospective_target: Bits<32> = if take_jalr_raw {
        jalr_target
    } else if take_jal_raw {
        jal_target
    } else {
        branch_target
    };
    let attempts_redirect: bool = take_jalr_raw || take_jal_raw || take_branch_raw;
    let target_misaligned: bool = (prospective_target & bits::<32>(0x3)) != bits::<32>(0);
    let take_misaligned: bool = attempts_redirect && target_misaligned;

    // Trap detection in Execute: ECALL / EBREAK / illegal
    // instruction (recognised by the decoder's `illegal` flag,
    // forwarded via id_ex.opcode == Illegal) / misaligned-target.
    // All four squash IF/ID, ID/EX, and EX/MEM-next; redirect PC
    // to mtvec; and signal the CSR file's trap port.  MRET is the
    // inverse: PC ← mepc; squashes the same three slots so the
    // trap handler's tail doesn't get re-executed.
    let take_ecall:   bool = q.id_ex.valid && q.id_ex.system_op == SystemOp::Ecall;
    let take_ebreak:  bool = q.id_ex.valid && q.id_ex.system_op == SystemOp::Ebreak;
    let take_illegal: bool = q.id_ex.valid && q.id_ex.opcode == Opcode::Illegal;
    let take_trap:    bool = take_ecall || take_ebreak || take_illegal || take_misaligned;
    let take_mret:    bool = q.id_ex.valid && q.id_ex.system_op == SystemOp::Mret;
    let trap_cause: Bits<32> = if take_misaligned {
        bits::<32>(0)
    } else if take_illegal {
        bits::<32>(2)
    } else if take_ebreak {
        bits::<32>(3)
    } else {
        bits::<32>(11)
    };
    let trap_val: Bits<32> = if take_misaligned {
        prospective_target
    } else {
        bits::<32>(0)
    };

    // Suppress branch/jump redirect when the same instruction also
    // takes a misaligned-target trap — the trap target wins.
    let take_branch: bool = take_branch_raw && !take_misaligned;
    let take_jal:    bool = take_jal_raw    && !take_misaligned;
    let take_jalr:   bool = take_jalr_raw   && !take_misaligned;

    // Redirect = branch / jump / trap / MRET.  Trap target is
    // mtvec; MRET target is mepc; branch / jump targets are
    // computed above.
    let redirect: bool = take_branch || take_jal || take_jalr || take_trap || take_mret;
    let redirect_target: Bits<32> = if take_trap {
        q.csrs.mtvec
    } else if take_mret {
        q.csrs.mepc
    } else if take_jalr {
        jalr_target
    } else if take_jal {
        jal_target
    } else {
        branch_target
    };

    // Pick alu_result for EX/MEM: for CSR-into-rd, carry the
    // pre-modify CSR value (the Memory stage's mux over
    // writeback_src steers it correctly).
    let ex_mem_alu_result: Bits<32> = if q.id_ex.writeback_src == WritebackSrc::Csr {
        csr_rdata
    } else {
        alu_result
    };

    // On a trap, the in-flight EX/MEM slot becomes a bubble — the
    // trap squashes the trapping instruction's writeback and any
    // memory side-effect.
    let next_ex_mem_real = ExMem {
        rd: q.id_ex.rd,
        alu_result: ex_mem_alu_result,
        rs2_val: rs2_fwd,
        mem_op: q.id_ex.mem_op,
        writeback_src: q.id_ex.writeback_src,
        mem_write: q.id_ex.valid && q.id_ex.mem_write,
        mem_read: q.id_ex.valid && q.id_ex.mem_read,
        pc_plus_4: pc_plus_4_ex,
        csr_addr: q.id_ex.csr_addr,
        csr_new_value,
        csr_writes: csr_writes_ex,
        valid: q.id_ex.valid,
    };
    // On a trap or MRET, the in-flight EX/MEM slot becomes a
    // bubble — the trap squashes the trapping instruction's
    // writeback and any memory side-effect; the MRET itself
    // doesn't write any register.
    let next_ex_mem: ExMem = if take_trap || take_mret {
        ExMem::default()
    } else {
        next_ex_mem_real
    };

    // ---- Decode stage (D) -------------------------------------------
    //
    // Decode IF/ID's instruction; read source registers via the
    // RegFile sub-widget; compute ID/EX-next.

    let dec = decode(q.if_id.instr);

    // Read register-file values via `q.rf` (driven from `d.rf`
    // below — combinational read ports).
    //
    // **WB→Decode bypass**: when MEM/WB is about to commit a
    // writeback to a register the Decode stage just read, the
    // regfile output is stale (the write commits at the cycle
    // edge, AFTER Decode reads).  Bypass the MEM/WB writeback
    // value here so Decode sees the post-commit value this cycle.
    // Without this, a 3-instructions-back dependency
    // (e.g. `add x12,…; lui …; addi …; bne x12,…`) reads the
    // stale x12 — EX/MEM and MEM/WB forwarding can only reach
    // back 1 and 2 instructions, not 3.
    let wb_bypass_x12: bool = q.mem_wb.valid
        && q.mem_wb.writeback_en
        && q.mem_wb.rd != bits::<5>(0);
    let raw_rs1: Bits<32> = q.rf.rdata1;
    let raw_rs2: Bits<32> = q.rf.rdata2;
    let id_rs1_val: Bits<32> = if wb_bypass_x12 && q.mem_wb.rd == dec.rs1 {
        q.mem_wb.writeback_value
    } else {
        raw_rs1
    };
    let id_rs2_val: Bits<32> = if wb_bypass_x12 && q.mem_wb.rd == dec.rs2 {
        q.mem_wb.writeback_value
    } else {
        raw_rs2
    };

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
        csr_op: dec.csr_op,
        csr_addr: dec.csr_addr,
        system_op: dec.system_op,
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

    // ---- Drive the CSR file's input port ----------------------------
    //
    // Read port: driven by Execute's csr_addr (so q.csrs.rdata
    // reflects the in-Execute CSR instruction's pre-modify value).
    // Write port: driven by MEM/WB's csr fields (commits the
    // CSRRW/CSRRS/CSRRC's new value).
    // Trap port: driven by Execute's trap signals (ECALL / EBREAK
    // → captures mepc / mcause / mtval atomically).
    d.csrs = CsrIn {
        raddr: q.id_ex.csr_addr,
        waddr: q.mem_wb.csr_addr,
        wdata: q.mem_wb.csr_new_value,
        wen: q.mem_wb.valid && q.mem_wb.csr_writes,
        trap_en: take_trap,
        trap_pc: q.id_ex.pc,
        trap_cause,
        trap_val,
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
        // Drive the CSR file to a quiescent input during reset
        // (the CSR file's own reset clears its registers).
        d.csrs = CsrIn::default();
        o.pc = bits::<32>(0);
        o.mem_write = false;
        o.mem_read = false;
    }
    (o, d)
}
