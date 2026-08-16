//! RV32I instruction decoder.
//!
//! Pure combinational kernel that takes a 32-bit instruction word
//! and produces a fully-specified [`DecodedInstruction`].
//!
//! The decoder distributes the work in three layers:
//!
//! 1. **Field extraction** — chop the 32-bit word into the named
//!    bit-field positions defined by the encoding type
//!    (R/I/S/B/U/J).  Pure shift-and-mask.
//! 2. **Immediate sign-extension** — assemble the immediate from
//!    its scattered bit positions and sign-extend it to 32 bits.
//!    The B-type and J-type encodings are particularly fragmented;
//!    each is handled in its own helper.
//! 3. **Opcode dispatch** — match on the 7-bit major opcode and
//!    fill in the [`DecodedInstruction`] fields appropriate for
//!    that opcode class.

use crate::isa::*;
use rhdl::prelude::*;

/// Sign-extend the I-type immediate (`instr[31:20]`) to 32 bits.
#[kernel]
pub fn imm_i(instr: Bits<32>) -> Bits<32> {
    // Extract bits 31:20 (12 bits) and sign-extend.  We use the
    // signed shift-right trick: shift left to put bit 31 of the
    // immediate at bit 31 of the word, then arithmetic-shift right.
    let raw: SignedBits<32> = instr.as_signed();
    let shifted_left: SignedBits<32> = raw >> 20;
    shifted_left.as_unsigned()
}

/// Sign-extend the S-type immediate (`instr[31:25] | instr[11:7]`).
#[kernel]
pub fn imm_s(instr: Bits<32>) -> Bits<32> {
    // imm[11:5] = instr[31:25], imm[4:0] = instr[11:7]
    let high: Bits<32> = (instr >> 25) << 5; // bits 11:5 of imm
    let low: Bits<32> = (instr >> 7) & bits::<32>(0x1F); // bits 4:0 of imm
    let combined: Bits<32> = high | low;
    // Sign-extend by checking bit 11 (instr[31]).
    let sign_mask: Bits<32> = if (instr >> 31) & bits::<32>(1) != bits::<32>(0) {
        bits::<32>(0xFFFF_F000)
    } else {
        bits::<32>(0)
    };
    combined | sign_mask
}

/// Sign-extend the B-type immediate (split across instr[31], [7],
/// [30:25], [11:8]; bit 0 is always 0).
#[kernel]
pub fn imm_b(instr: Bits<32>) -> Bits<32> {
    // imm[12]    = instr[31]
    // imm[11]    = instr[7]
    // imm[10:5]  = instr[30:25]
    // imm[4:1]   = instr[11:8]
    // imm[0]     = 0
    let bit12: Bits<32> = ((instr >> 31) & bits::<32>(1)) << 12;
    let bit11: Bits<32> = ((instr >> 7) & bits::<32>(1)) << 11;
    let bits_10_5: Bits<32> = ((instr >> 25) & bits::<32>(0x3F)) << 5;
    let bits_4_1: Bits<32> = ((instr >> 8) & bits::<32>(0xF)) << 1;
    let combined: Bits<32> = bit12 | bit11 | bits_10_5 | bits_4_1;
    // Sign-extend from bit 12.
    let sign_mask: Bits<32> = if (instr >> 31) & bits::<32>(1) != bits::<32>(0) {
        bits::<32>(0xFFFF_E000)
    } else {
        bits::<32>(0)
    };
    combined | sign_mask
}

/// U-type immediate is `instr[31:12]` placed at bits 31:12 of
/// the result (bits 11:0 are zero).  No sign extension required.
#[kernel]
pub fn imm_u(instr: Bits<32>) -> Bits<32> {
    instr & bits::<32>(0xFFFF_F000)
}

/// J-type immediate (split across instr[31], [19:12], [20], [30:21];
/// bit 0 is always 0).  Sign-extended to 32 bits.
#[kernel]
pub fn imm_j(instr: Bits<32>) -> Bits<32> {
    // imm[20]    = instr[31]
    // imm[19:12] = instr[19:12]
    // imm[11]    = instr[20]
    // imm[10:1]  = instr[30:21]
    // imm[0]     = 0
    let bit20: Bits<32> = ((instr >> 31) & bits::<32>(1)) << 20;
    let bits_19_12: Bits<32> = ((instr >> 12) & bits::<32>(0xFF)) << 12;
    let bit11: Bits<32> = ((instr >> 20) & bits::<32>(1)) << 11;
    let bits_10_1: Bits<32> = ((instr >> 21) & bits::<32>(0x3FF)) << 1;
    let combined: Bits<32> = bit20 | bits_19_12 | bit11 | bits_10_1;
    let sign_mask: Bits<32> = if (instr >> 31) & bits::<32>(1) != bits::<32>(0) {
        bits::<32>(0xFFE0_0000)
    } else {
        bits::<32>(0)
    };
    combined | sign_mask
}

/// Decode a 32-bit RV32I instruction word.  Pure combinational
/// kernel — no clock, no state.  Output is fully specified for
/// every input (illegal opcodes produce an `illegal: true` flag).
#[kernel]
pub fn decode(instr: Bits<32>) -> DecodedInstruction {
    // Field extraction.
    let opcode_bits: Bits<7> = (instr & bits::<32>(0x7F)).resize();
    let rd: Bits<5> = ((instr >> 7) & bits::<32>(0x1F)).resize();
    let funct3: Bits<3> = ((instr >> 12) & bits::<32>(0x7)).resize();
    let rs1: Bits<5> = ((instr >> 15) & bits::<32>(0x1F)).resize();
    let rs2: Bits<5> = ((instr >> 20) & bits::<32>(0x1F)).resize();
    let funct7: Bits<7> = ((instr >> 25) & bits::<32>(0x7F)).resize();

    // Sign-extended immediates per encoding type.
    let imm_i_v = imm_i(instr);
    let imm_s_v = imm_s(instr);
    let imm_b_v = imm_b(instr);
    let imm_u_v = imm_u(instr);
    let imm_j_v = imm_j(instr);

    // CSR address = funct12 = instr[31:20] (the upper 12 bits).
    let csr_addr: Bits<12> = ((instr >> 20) & bits::<32>(0xFFF)).resize();

    // Default-case decoded instruction (illegal).
    let mut d = DecodedInstruction {
        opcode: Opcode::Illegal,
        rd,
        rs1,
        rs2,
        imm: bits::<32>(0),
        alu_op: AluOp::Add,
        alu_src: AluSrc::Reg,
        branch_op: BranchOp::Eq,
        mem_op: MemOp::Lw,
        writeback_src: WritebackSrc::None,
        mem_write: false,
        mem_read: false,
        jump: false,
        is_jalr: false,
        illegal: true,
        csr_op: CsrOp::None,
        csr_addr,
        system_op: SystemOp::None,
    };

    // Opcode dispatch.  RV32I major opcodes are 7-bit values; we
    // match on the literal bit patterns from the spec.
    let op_reg: Bits<7> = bits::<7>(0b0110011);
    let op_imm_v: Bits<7> = bits::<7>(0b0010011);
    let op_load: Bits<7> = bits::<7>(0b0000011);
    let op_store: Bits<7> = bits::<7>(0b0100011);
    let op_branch: Bits<7> = bits::<7>(0b1100011);
    let op_jalr: Bits<7> = bits::<7>(0b1100111);
    let op_jal: Bits<7> = bits::<7>(0b1101111);
    let op_lui: Bits<7> = bits::<7>(0b0110111);
    let op_auipc: Bits<7> = bits::<7>(0b0010111);
    let op_system: Bits<7> = bits::<7>(0b1110011);
    let op_misc_mem: Bits<7> = bits::<7>(0b0001111);

    let f3_zero: Bits<3> = bits::<3>(0);
    let f3_one: Bits<3> = bits::<3>(1);
    let f3_two: Bits<3> = bits::<3>(2);
    let f3_three: Bits<3> = bits::<3>(3);
    let f3_four: Bits<3> = bits::<3>(4);
    let f3_five: Bits<3> = bits::<3>(5);
    let f3_six: Bits<3> = bits::<3>(6);
    let f3_seven: Bits<3> = bits::<3>(7);

    let f7_alt: Bits<7> = bits::<7>(0b0100000); // SUB, SRA(I)

    if opcode_bits == op_reg {
        // R-type ALU.  funct3 selects the operation; funct7 bit 5
        // distinguishes ADD/SUB and SRL/SRA.
        d.opcode = Opcode::OpReg;
        d.alu_src = AluSrc::Reg;
        d.writeback_src = WritebackSrc::Alu;
        d.illegal = false;
        if funct3 == f3_zero {
            d.alu_op = if funct7 == f7_alt {
                AluOp::Sub
            } else {
                AluOp::Add
            };
        } else if funct3 == f3_one {
            d.alu_op = AluOp::Sll;
        } else if funct3 == f3_two {
            d.alu_op = AluOp::Slt;
        } else if funct3 == f3_three {
            d.alu_op = AluOp::Sltu;
        } else if funct3 == f3_four {
            d.alu_op = AluOp::Xor;
        } else if funct3 == f3_five {
            d.alu_op = if funct7 == f7_alt {
                AluOp::Sra
            } else {
                AluOp::Srl
            };
        } else if funct3 == f3_six {
            d.alu_op = AluOp::Or;
        } else {
            d.alu_op = AluOp::And;
        }
    } else if opcode_bits == op_imm_v {
        // I-type ALU.  Same funct3 mapping; funct7 only checked
        // for shifts (shift-amount immediate is in rs2, funct7
        // selects logical/arithmetic for SRLI/SRAI).
        d.opcode = Opcode::OpImm;
        d.alu_src = AluSrc::Imm;
        d.imm = imm_i_v;
        d.writeback_src = WritebackSrc::Alu;
        d.illegal = false;
        if funct3 == f3_zero {
            d.alu_op = AluOp::Add;
        } else if funct3 == f3_one {
            d.alu_op = AluOp::Sll;
        } else if funct3 == f3_two {
            d.alu_op = AluOp::Slt;
        } else if funct3 == f3_three {
            d.alu_op = AluOp::Sltu;
        } else if funct3 == f3_four {
            d.alu_op = AluOp::Xor;
        } else if funct3 == f3_five {
            d.alu_op = if funct7 == f7_alt {
                AluOp::Sra
            } else {
                AluOp::Srl
            };
        } else if funct3 == f3_six {
            d.alu_op = AluOp::Or;
        } else {
            d.alu_op = AluOp::And;
        }
    } else if opcode_bits == op_load {
        // I-type load.
        d.opcode = Opcode::Load;
        d.alu_src = AluSrc::Imm;
        d.alu_op = AluOp::Add; // address = rs1 + imm
        d.imm = imm_i_v;
        d.writeback_src = WritebackSrc::Mem;
        d.mem_read = true;
        d.illegal = false;
        if funct3 == f3_zero {
            d.mem_op = MemOp::Lb;
        } else if funct3 == f3_one {
            d.mem_op = MemOp::Lh;
        } else if funct3 == f3_two {
            d.mem_op = MemOp::Lw;
        } else if funct3 == f3_four {
            d.mem_op = MemOp::Lbu;
        } else if funct3 == f3_five {
            d.mem_op = MemOp::Lhu;
        } else {
            d.illegal = true;
        }
    } else if opcode_bits == op_store {
        // S-type store.
        d.opcode = Opcode::Store;
        d.alu_src = AluSrc::Imm;
        d.alu_op = AluOp::Add; // address = rs1 + imm
        d.imm = imm_s_v;
        d.mem_write = true;
        d.illegal = false;
        if funct3 == f3_zero {
            d.mem_op = MemOp::Sb;
        } else if funct3 == f3_one {
            d.mem_op = MemOp::Sh;
        } else if funct3 == f3_two {
            d.mem_op = MemOp::Sw;
        } else {
            d.illegal = true;
        }
    } else if opcode_bits == op_branch {
        // B-type branch.
        d.opcode = Opcode::Branch;
        d.alu_src = AluSrc::Reg; // branch comparator uses both regs
        d.imm = imm_b_v;
        d.illegal = false;
        if funct3 == f3_zero {
            d.branch_op = BranchOp::Eq;
        } else if funct3 == f3_one {
            d.branch_op = BranchOp::Ne;
        } else if funct3 == f3_four {
            d.branch_op = BranchOp::Lt;
        } else if funct3 == f3_five {
            d.branch_op = BranchOp::Ge;
        } else if funct3 == f3_six {
            d.branch_op = BranchOp::Ltu;
        } else if funct3 == f3_seven {
            d.branch_op = BranchOp::Geu;
        } else {
            d.illegal = true;
        }
    } else if opcode_bits == op_jalr {
        // I-type JALR.
        d.opcode = Opcode::Jalr;
        d.alu_src = AluSrc::Imm;
        d.alu_op = AluOp::Add; // target = rs1 + imm
        d.imm = imm_i_v;
        d.writeback_src = WritebackSrc::PcPlus4;
        d.jump = true;
        d.is_jalr = true;
        d.illegal = false;
    } else if opcode_bits == op_jal {
        // J-type JAL.
        d.opcode = Opcode::Jal;
        d.imm = imm_j_v;
        d.writeback_src = WritebackSrc::PcPlus4;
        d.jump = true;
        d.illegal = false;
    } else if opcode_bits == op_lui {
        // U-type LUI: rd = imm.
        d.opcode = Opcode::Lui;
        d.alu_src = AluSrc::Imm;
        d.alu_op = AluOp::Pass; // ALU passes second operand
        d.imm = imm_u_v;
        d.writeback_src = WritebackSrc::Alu;
        d.illegal = false;
    } else if opcode_bits == op_auipc {
        // U-type AUIPC: rd = PC + imm.  The "PC" feeds in via
        // executor wiring (rs1 isn't used; we route PC as op-A).
        d.opcode = Opcode::Auipc;
        d.alu_src = AluSrc::Imm;
        d.alu_op = AluOp::Add;
        d.imm = imm_u_v;
        d.writeback_src = WritebackSrc::Alu;
        d.illegal = false;
    } else if opcode_bits == op_system {
        // SYSTEM opcode encodes ECALL / EBREAK / CSR access.
        // funct3 distinguishes:
        //   0 → ECALL (funct12 = 0) / EBREAK (funct12 = 1)
        //   1 → CSRRW    2 → CSRRS    3 → CSRRC
        //   5 → CSRRWI   6 → CSRRSI   7 → CSRRCI
        //   4 is reserved (illegal).
        d.opcode = Opcode::System;
        d.illegal = false;
        if funct3 == f3_zero {
            // SYSTEM-funct12 dispatch:
            //   0x000 → ECALL
            //   0x001 → EBREAK
            //   0x302 → MRET   (M-mode return-from-trap)
            //   0x105 → WFI    (wait for interrupt; NOP w/o interrupts)
            //   anything else → illegal (SFENCE.VMA, SRET, URET,
            //                            DRET, etc. not implemented).
            let funct12: Bits<12> = ((instr >> 20) & bits::<32>(0xFFF)).resize();
            if funct12 == bits::<12>(0) {
                d.system_op = SystemOp::Ecall;
            } else if funct12 == bits::<12>(1) {
                d.system_op = SystemOp::Ebreak;
            } else if funct12 == bits::<12>(0x302) {
                d.system_op = SystemOp::Mret;
            } else if funct12 == bits::<12>(0x105) {
                d.system_op = SystemOp::Wfi;
            } else {
                d.illegal = true;
            }
        } else {
            // CSR instruction.  Always reads the CSR into rd
            // (writeback_src = Csr, the executor selects the
            // pre-modify CSR value as the writeback).
            d.writeback_src = WritebackSrc::Csr;
            if funct3 == f3_one {
                d.csr_op = CsrOp::ReadWrite;
            } else if funct3 == f3_two {
                d.csr_op = CsrOp::ReadSet;
            } else if funct3 == f3_three {
                d.csr_op = CsrOp::ReadClear;
            } else if funct3 == f3_five {
                d.csr_op = CsrOp::ReadWriteImm;
            } else if funct3 == f3_six {
                d.csr_op = CsrOp::ReadSetImm;
            } else if funct3 == f3_seven {
                d.csr_op = CsrOp::ReadClearImm;
            } else {
                d.illegal = true;
            }
        }
    } else if opcode_bits == op_misc_mem {
        // FENCE — treated as NOP in this single-hart in-order core.
        d.opcode = Opcode::MiscMem;
        d.illegal = false;
    }

    d
}
