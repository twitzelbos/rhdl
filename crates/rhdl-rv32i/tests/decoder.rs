//! Decoder unit tests — one per RV32I instruction class.
//!
//! Each test constructs an instruction word from its field values
//! per the spec's encoding diagrams and asserts on the resulting
//! `DecodedInstruction`.

use rhdl::prelude::*;
use rhdl_rv32i::decoder::*;
use rhdl_rv32i::isa::*;

// ---- Encoding helpers --------------------------------------------

/// Build an R-type instruction word.
fn r_type(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> Bits<32> {
    bits::<32>(((funct7 & 0x7F) << 25
        | (rs2 & 0x1F) << 20
        | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12
        | (rd & 0x1F) << 7
        | (opcode & 0x7F)) as u128)
}

/// Build an I-type instruction word.
fn i_type(imm: i32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> Bits<32> {
    let imm_u = (imm as u32) & 0xFFF;
    bits::<32>(((imm_u << 20)
        | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12
        | (rd & 0x1F) << 7
        | (opcode & 0x7F)) as u128)
}

/// Build an S-type instruction word.
fn s_type(imm: i32, rs2: u32, rs1: u32, funct3: u32, opcode: u32) -> Bits<32> {
    let imm_u = (imm as u32) & 0xFFF;
    let imm_high = (imm_u >> 5) & 0x7F;
    let imm_low = imm_u & 0x1F;
    bits::<32>(((imm_high << 25)
        | (rs2 & 0x1F) << 20
        | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12
        | imm_low << 7
        | (opcode & 0x7F)) as u128)
}

/// Build a B-type instruction word.
fn b_type(imm: i32, rs2: u32, rs1: u32, funct3: u32, opcode: u32) -> Bits<32> {
    let imm_u = (imm as u32) & 0x1FFF;
    let bit12 = (imm_u >> 12) & 1;
    let bit11 = (imm_u >> 11) & 1;
    let bits_10_5 = (imm_u >> 5) & 0x3F;
    let bits_4_1 = (imm_u >> 1) & 0xF;
    bits::<32>(((bit12 << 31)
        | bits_10_5 << 25
        | (rs2 & 0x1F) << 20
        | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12
        | bits_4_1 << 8
        | bit11 << 7
        | (opcode & 0x7F)) as u128)
}

/// Build a U-type instruction word.
fn u_type(imm: u32, rd: u32, opcode: u32) -> Bits<32> {
    bits::<32>(((imm & 0xFFFF_F000)
        | (rd & 0x1F) << 7
        | (opcode & 0x7F)) as u128)
}

/// Build a J-type instruction word.
fn j_type(imm: i32, rd: u32, opcode: u32) -> Bits<32> {
    let imm_u = (imm as u32) & 0x1F_FFFF;
    let bit20 = (imm_u >> 20) & 1;
    let bits_19_12 = (imm_u >> 12) & 0xFF;
    let bit11 = (imm_u >> 11) & 1;
    let bits_10_1 = (imm_u >> 1) & 0x3FF;
    bits::<32>(((bit20 << 31)
        | bits_10_1 << 21
        | bit11 << 20
        | bits_19_12 << 12
        | (rd & 0x1F) << 7
        | (opcode & 0x7F)) as u128)
}

// ---- R-type ALU instructions -------------------------------------

#[test]
fn decode_add() {
    // ADD x5, x6, x7  → funct7=0, funct3=0, opcode=0x33
    let d = decode(r_type(0, 7, 6, 0, 5, 0x33));
    assert_eq!(d.opcode, Opcode::OpReg);
    assert_eq!(d.alu_op, AluOp::Add);
    assert_eq!(d.alu_src, AluSrc::Reg);
    assert_eq!(d.rd, bits::<5>(5));
    assert_eq!(d.rs1, bits::<5>(6));
    assert_eq!(d.rs2, bits::<5>(7));
    assert_eq!(d.writeback_src, WritebackSrc::Alu);
    assert!(!d.illegal);
}

#[test]
fn decode_sub() {
    // SUB x5, x6, x7  → funct7=0x20, funct3=0, opcode=0x33
    let d = decode(r_type(0x20, 7, 6, 0, 5, 0x33));
    assert_eq!(d.alu_op, AluOp::Sub);
}

#[test]
fn decode_all_r_type_alu_ops() {
    // funct3 → AluOp mapping (when funct7 = 0).
    for (funct3, op) in [
        (0, AluOp::Add),
        (1, AluOp::Sll),
        (2, AluOp::Slt),
        (3, AluOp::Sltu),
        (4, AluOp::Xor),
        (5, AluOp::Srl),
        (6, AluOp::Or),
        (7, AluOp::And),
    ] {
        let d = decode(r_type(0, 7, 6, funct3, 5, 0x33));
        assert_eq!(d.alu_op, op, "funct3 = {funct3} should map to {op:?}");
    }
    // funct7=0x20 toggles SUB and SRA.
    let d = decode(r_type(0x20, 7, 6, 0, 5, 0x33));
    assert_eq!(d.alu_op, AluOp::Sub);
    let d = decode(r_type(0x20, 7, 6, 5, 5, 0x33));
    assert_eq!(d.alu_op, AluOp::Sra);
}

// ---- I-type ALU instructions -------------------------------------

#[test]
fn decode_addi() {
    // ADDI x5, x6, 100  → opcode=0x13, funct3=0
    let d = decode(i_type(100, 6, 0, 5, 0x13));
    assert_eq!(d.opcode, Opcode::OpImm);
    assert_eq!(d.alu_op, AluOp::Add);
    assert_eq!(d.alu_src, AluSrc::Imm);
    assert_eq!(d.imm, bits::<32>(100));
    assert_eq!(d.rd, bits::<5>(5));
    assert_eq!(d.rs1, bits::<5>(6));
    assert!(!d.illegal);
}

#[test]
fn decode_addi_negative_immediate_sign_extends() {
    // ADDI x5, x6, -1  → imm = 0xFFF, sign-extended to 0xFFFF_FFFF
    let d = decode(i_type(-1, 6, 0, 5, 0x13));
    assert_eq!(d.imm, bits::<32>(0xFFFF_FFFF));
}

// ---- Loads -------------------------------------------------------

#[test]
fn decode_lw() {
    // LW x5, 4(x6)  → opcode=0x03, funct3=2
    let d = decode(i_type(4, 6, 2, 5, 0x03));
    assert_eq!(d.opcode, Opcode::Load);
    assert_eq!(d.mem_op, MemOp::Lw);
    assert!(d.mem_read);
    assert!(!d.mem_write);
    assert_eq!(d.writeback_src, WritebackSrc::Mem);
    assert_eq!(d.imm, bits::<32>(4));
}

#[test]
fn decode_all_load_ops() {
    for (funct3, op) in [
        (0, MemOp::Lb),
        (1, MemOp::Lh),
        (2, MemOp::Lw),
        (4, MemOp::Lbu),
        (5, MemOp::Lhu),
    ] {
        let d = decode(i_type(0, 6, funct3, 5, 0x03));
        assert_eq!(d.mem_op, op);
        assert!(d.mem_read);
    }
    // funct3 = 3, 6, 7 are illegal (no LD/LWU/<undef> in RV32I).
    let d = decode(i_type(0, 6, 3, 5, 0x03));
    assert!(d.illegal);
}

// ---- Stores ------------------------------------------------------

#[test]
fn decode_sw() {
    // SW x5, 8(x6)  → opcode=0x23, funct3=2, rs2=5, rs1=6, imm=8
    let d = decode(s_type(8, 5, 6, 2, 0x23));
    assert_eq!(d.opcode, Opcode::Store);
    assert_eq!(d.mem_op, MemOp::Sw);
    assert!(d.mem_write);
    assert!(!d.mem_read);
    assert_eq!(d.rs1, bits::<5>(6));
    assert_eq!(d.rs2, bits::<5>(5));
    assert_eq!(d.imm, bits::<32>(8));
}

#[test]
fn decode_sw_negative_offset_sign_extends() {
    // SW x5, -4(x6)  → imm = 0xFFC, sign-extends to 0xFFFF_FFFC
    let d = decode(s_type(-4, 5, 6, 2, 0x23));
    assert_eq!(d.imm, bits::<32>(0xFFFF_FFFC));
}

// ---- Branches ----------------------------------------------------

#[test]
fn decode_beq() {
    // BEQ x5, x6, +8  → opcode=0x63, funct3=0
    let d = decode(b_type(8, 6, 5, 0, 0x63));
    assert_eq!(d.opcode, Opcode::Branch);
    assert_eq!(d.branch_op, BranchOp::Eq);
    assert_eq!(d.imm, bits::<32>(8));
    assert!(!d.jump);     // jump flag is for unconditional control flow
}

#[test]
fn decode_all_branch_ops() {
    for (funct3, op) in [
        (0, BranchOp::Eq),
        (1, BranchOp::Ne),
        (4, BranchOp::Lt),
        (5, BranchOp::Ge),
        (6, BranchOp::Ltu),
        (7, BranchOp::Geu),
    ] {
        let d = decode(b_type(0, 6, 5, funct3, 0x63));
        assert_eq!(d.branch_op, op);
    }
    // funct3 = 2, 3 are illegal in RV32I.
    let d = decode(b_type(0, 6, 5, 2, 0x63));
    assert!(d.illegal);
}

// ---- Jumps -------------------------------------------------------

#[test]
fn decode_jal() {
    // JAL x1, +16  → opcode=0x6F
    let d = decode(j_type(16, 1, 0x6F));
    assert_eq!(d.opcode, Opcode::Jal);
    assert_eq!(d.rd, bits::<5>(1));
    assert_eq!(d.imm, bits::<32>(16));
    assert!(d.jump);
    assert!(!d.is_jalr);
    assert_eq!(d.writeback_src, WritebackSrc::PcPlus4);
}

#[test]
fn decode_jalr() {
    // JALR x1, x2, 12  → opcode=0x67, funct3=0
    let d = decode(i_type(12, 2, 0, 1, 0x67));
    assert_eq!(d.opcode, Opcode::Jalr);
    assert_eq!(d.rd, bits::<5>(1));
    assert_eq!(d.rs1, bits::<5>(2));
    assert_eq!(d.imm, bits::<32>(12));
    assert!(d.jump);
    assert!(d.is_jalr);
    assert_eq!(d.writeback_src, WritebackSrc::PcPlus4);
}

// ---- Upper-immediate ---------------------------------------------

#[test]
fn decode_lui() {
    // LUI x5, 0x12345  → opcode=0x37
    let d = decode(u_type(0x12345 << 12, 5, 0x37));
    assert_eq!(d.opcode, Opcode::Lui);
    assert_eq!(d.alu_op, AluOp::Pass);
    assert_eq!(d.imm, bits::<32>(0x12345 << 12));
}

#[test]
fn decode_auipc() {
    // AUIPC x5, 0x12345  → opcode=0x17
    let d = decode(u_type(0x12345 << 12, 5, 0x17));
    assert_eq!(d.opcode, Opcode::Auipc);
    assert_eq!(d.alu_op, AluOp::Add);
    assert_eq!(d.imm, bits::<32>(0x12345 << 12));
}

// ---- System / FENCE ----------------------------------------------

#[test]
fn decode_ecall() {
    // ECALL  →  0x00000073
    let d = decode(bits::<32>(0x0000_0073));
    assert_eq!(d.opcode, Opcode::System);
    assert!(!d.illegal);
}

#[test]
fn decode_ebreak() {
    // EBREAK  →  0x00100073
    let d = decode(bits::<32>(0x0010_0073));
    assert_eq!(d.opcode, Opcode::System);
    assert!(!d.illegal);
}

#[test]
fn decode_fence() {
    // FENCE  →  0x0000000F (with all predecessor/successor bits zero — NOP equivalent)
    let d = decode(bits::<32>(0x0000_000F));
    assert_eq!(d.opcode, Opcode::MiscMem);
    assert!(!d.illegal);
}

// ---- Illegal -----------------------------------------------------

#[test]
fn decode_unknown_opcode_is_illegal() {
    // opcode = 0x7F is not assigned in RV32I.
    let d = decode(bits::<32>(0x0000_007F));
    assert!(d.illegal);
}
