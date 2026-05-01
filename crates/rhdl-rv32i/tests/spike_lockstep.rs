//! Spike (riscv-isa-sim) lockstep tests.
//!
//! Runs each test program through the official RISC-V ISA reference
//! simulator (Spike) AND through both of our RHDL hardware cores;
//! asserts that the **final memory state at the data window** is
//! identical across all three implementations.
//!
//! ## Why Spike specifically
//!
//! Our in-house Rust simulator (`sim::Cpu`) is independent enough
//! to catch most bugs, but it shares the decoder with the hardware
//! — any decoder bug hides in both.  Spike has its own decoder,
//! its own execution engine, and is the official reference used by
//! the RISC-V Foundation for compliance work.  Disagreement
//! between Spike and our hardware is much more likely to be a
//! real bug than disagreement between two of our own crates.
//!
//! ## How it works
//!
//! 1. Test programs are written using the same encoding helpers as
//!    the rest of `tests/`.
//! 2. We compile each program into a minimal RV32I ELF in-memory
//!    (rolled-our-own ELF builder; no external dep) with code at
//!    `0x80000000` (Spike's default RAM base) and a 64-byte data
//!    window at `0x80001000` that the program writes to.
//! 3. Programs end with a HALT pattern (`beq x0, x0, +0` — same as
//!    the rest of our suite).  Spike runs `step N` instructions
//!    via `--debug-cmd`, then dumps the data window via `mem`.
//! 4. Our hardware runs the same program (translated to use
//!    `0x80000000` as the program-fetch base — though our harness
//!    actually uses `program[pc/4]` so the load address is
//!    immaterial; we compare the data-window writes directly).
//! 5. Compare the data window (8 words) — expect agreement.
//!
//! ## Skip behaviour
//!
//! If `spike` is not on PATH, every test in this file is skipped
//! with a printed message.  To enable: build `riscv-isa-sim` from
//! source (see <https://github.com/riscv-software-src/riscv-isa-sim>)
//! and put `spike` on PATH.

use rhdl::core::sim::ResetOrData;
use rhdl::prelude::*;
use rhdl_rv32i::cpu::{Cpu, In as SInIn, Out as SOut};
use rhdl_rv32i::pipelined::{In as PIn, Out as POut, PipelinedCpu};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

// ---- Spike availability detection --------------------------------

fn spike_path() -> Option<PathBuf> {
    // Try $PATH first; fall back to a known build location used by
    // our build script (see CHANGELOG for the install instructions).
    if let Ok(out) = Command::new("which").arg("spike").output() {
        if out.status.success() {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }
    // Try the local-build install path used by this repo's CI.
    let local = PathBuf::from("/tmp/spike-install/bin/spike");
    if local.exists() {
        return Some(local);
    }
    None
}

fn require_spike() -> Option<PathBuf> {
    let path = spike_path();
    if path.is_none() {
        eprintln!("spike not found on PATH or at /tmp/spike-install/bin/spike — skipping");
        eprintln!("  to enable: build riscv-isa-sim from source (https://github.com/riscv-software-src/riscv-isa-sim)");
    }
    path
}

// ---- Minimal RV32 ELF builder ------------------------------------

const ELF_LOAD_ADDR: u32 = 0x8000_0000;

/// Write a 32-bit ELF section header (40 bytes) into `buf`.
fn shdr(buf: &mut Vec<u8>,
        sh_name: u32, sh_type: u32, sh_flags: u32, sh_addr: u32,
        sh_offset: u32, sh_size: u32, sh_link: u32, sh_info: u32,
        sh_addralign: u32, sh_entsize: u32) {
    buf.extend_from_slice(&sh_name.to_le_bytes());
    buf.extend_from_slice(&sh_type.to_le_bytes());
    buf.extend_from_slice(&sh_flags.to_le_bytes());
    buf.extend_from_slice(&sh_addr.to_le_bytes());
    buf.extend_from_slice(&sh_offset.to_le_bytes());
    buf.extend_from_slice(&sh_size.to_le_bytes());
    buf.extend_from_slice(&sh_link.to_le_bytes());
    buf.extend_from_slice(&sh_info.to_le_bytes());
    buf.extend_from_slice(&sh_addralign.to_le_bytes());
    buf.extend_from_slice(&sh_entsize.to_le_bytes());
}

/// Build a minimal RV32 little-endian ELF executable.  One PT_LOAD
/// segment (RWX) holds `code` at `ELF_LOAD_ADDR`.  Three sections:
///
/// - `[0]` SHT_NULL (required by ELF spec)
/// - `[1]` `.text` (SHT_PROGBITS) — the code
/// - `[2]` `.shstrtab` (SHT_STRTAB) — section-name string table
///
/// The strtab section is required because Spike asserts
/// `sh[i].sh_name < sh[e_shstrndx].sh_size` for every section.
fn build_elf(code: &[u32]) -> Vec<u8> {
    let mut buf = Vec::new();
    let code_bytes_len = (code.len() * 4) as u32;
    let phdr_offset: u32 = 52;
    let code_offset: u32 = 52 + 32;
    // String table contents: "\0.text\0.shstrtab\0"
    //   offset 0: "" (for SHT_NULL)
    //   offset 1: ".text"
    //   offset 7: ".shstrtab"
    let strtab: &[u8] = b"\0.text\0.shstrtab\0";
    let strtab_offset: u32 = code_offset + code_bytes_len;
    let strtab_size: u32 = strtab.len() as u32;
    let shdr_offset: u32 = strtab_offset + strtab_size;

    // ---- ELF header (52 bytes) ----
    buf.extend_from_slice(&[0x7F, b'E', b'L', b'F']);
    buf.push(1);  // EI_CLASS = ELFCLASS32
    buf.push(1);  // EI_DATA  = ELFDATA2LSB
    buf.push(1);  // EI_VERSION
    buf.push(0);  // EI_OSABI = SYSV
    buf.extend_from_slice(&[0u8; 8]);  // padding
    buf.extend_from_slice(&2u16.to_le_bytes());      // e_type = ET_EXEC
    buf.extend_from_slice(&0xF3u16.to_le_bytes());   // e_machine = EM_RISCV
    buf.extend_from_slice(&1u32.to_le_bytes());      // e_version
    buf.extend_from_slice(&ELF_LOAD_ADDR.to_le_bytes());  // e_entry
    buf.extend_from_slice(&phdr_offset.to_le_bytes());    // e_phoff
    buf.extend_from_slice(&shdr_offset.to_le_bytes());    // e_shoff
    buf.extend_from_slice(&0u32.to_le_bytes());      // e_flags
    buf.extend_from_slice(&52u16.to_le_bytes());     // e_ehsize
    buf.extend_from_slice(&32u16.to_le_bytes());     // e_phentsize
    buf.extend_from_slice(&1u16.to_le_bytes());      // e_phnum
    buf.extend_from_slice(&40u16.to_le_bytes());     // e_shentsize
    buf.extend_from_slice(&3u16.to_le_bytes());      // e_shnum
    buf.extend_from_slice(&2u16.to_le_bytes());      // e_shstrndx = section [2]

    // ---- Program header (32 bytes) ----
    let memsz: u32 = 0x10_0000;  // 1 MiB load region
    buf.extend_from_slice(&1u32.to_le_bytes());      // p_type = PT_LOAD
    buf.extend_from_slice(&code_offset.to_le_bytes()); // p_offset
    buf.extend_from_slice(&ELF_LOAD_ADDR.to_le_bytes()); // p_vaddr
    buf.extend_from_slice(&ELF_LOAD_ADDR.to_le_bytes()); // p_paddr
    buf.extend_from_slice(&code_bytes_len.to_le_bytes()); // p_filesz
    buf.extend_from_slice(&memsz.to_le_bytes());     // p_memsz
    buf.extend_from_slice(&7u32.to_le_bytes());      // p_flags = RWX
    buf.extend_from_slice(&0x1000u32.to_le_bytes()); // p_align

    // ---- Code ----
    for &word in code {
        buf.extend_from_slice(&word.to_le_bytes());
    }
    // ---- String table ----
    buf.extend_from_slice(strtab);

    // ---- Section headers ----
    // [0] SHT_NULL — all zeros (required).
    shdr(&mut buf, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
    // [1] .text — SHT_PROGBITS, ALLOC|EXEC, at ELF_LOAD_ADDR.
    shdr(&mut buf,
         1,                                          // sh_name = ".text"
         1,                                          // SHT_PROGBITS
         0x6,                                        // SHF_ALLOC | SHF_EXECINSTR
         ELF_LOAD_ADDR,
         code_offset,
         code_bytes_len,
         0, 0, 4, 0);
    // [2] .shstrtab — SHT_STRTAB, no flags, in-file only.
    shdr(&mut buf,
         7,                                          // sh_name = ".shstrtab"
         3,                                          // SHT_STRTAB
         0,
         0,
         strtab_offset,
         strtab_size,
         0, 0, 1, 0);

    buf
}

// ---- Spike runner -------------------------------------------------

/// Run `program` on Spike.  Runs until the HALT instruction's PC is
/// reached (so timing is independent of Spike's boot ROM), then
/// dumps the 8-word data window at `0x80001000`.
fn run_spike(spike: &PathBuf, program: &[u32], _cycles: u32) -> Option<[u32; 8]> {
    let elf_bytes = build_elf(program);
    let tmpdir = std::env::temp_dir();
    // Unique per-test path: use process ID + thread ID + nanos so
    // parallel test threads don't race on the same file.
    let tag = format!("{}-{:?}-{}",
        std::process::id(),
        std::thread::current().id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0));
    let elf_path = tmpdir.join(format!("rhdl-rv32i-spike-{tag}.elf"));
    let cmd_path = tmpdir.join(format!("rhdl-rv32i-spike-{tag}.cmd"));

    {
        let mut f = std::fs::File::create(&elf_path).ok()?;
        f.write_all(&elf_bytes).ok()?;
    }

    // HALT is the LAST instruction in `program`.  Its address is
    // ELF_LOAD_ADDR + (program.len() - 1) * 4.
    let halt_addr = ELF_LOAD_ADDR + ((program.len() - 1) as u32) * 4;
    // Debug commands:
    //   `untiln pc 0 ADDR` — run silently until hart 0's PC == ADDR.
    //   `mem ADDR`         — dump 32-bit word at ADDR (`0xXXXXXXXX`).
    //   `q`                — quit.
    let cmds = format!(
        "untiln pc 0 0x{:08x}\nmem 0x80001000\nmem 0x80001004\nmem 0x80001008\nmem 0x8000100c\nmem 0x80001010\nmem 0x80001014\nmem 0x80001018\nmem 0x8000101c\nq\n",
        halt_addr
    );
    {
        let mut f = std::fs::File::create(&cmd_path).ok()?;
        f.write_all(cmds.as_bytes()).ok()?;
    }

    let output = Command::new(spike)
        .arg("--isa=rv32i")
        .arg("-d")
        .arg(format!("--debug-cmd={}", cmd_path.display()))
        .arg(&elf_path)
        .output()
        .ok()?;

    let _ = std::fs::remove_file(&elf_path);
    let _ = std::fs::remove_file(&cmd_path);

    // Spike's `mem` output is an 8-character hex string per address,
    // one per line.  The output goes to stderr in debug mode.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stderr}{stdout}");

    let mut words = [0u32; 8];
    let mut idx = 0;
    for line in combined.lines() {
        let trimmed = line.trim();
        // Spike's mem dump format: "0xXXXXXXXX" (with 0x prefix).
        if let Some(hex) = trimmed.strip_prefix("0x") {
            if hex.len() == 8 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                if let Ok(v) = u32::from_str_radix(hex, 16) {
                    if idx < 8 {
                        words[idx] = v;
                        idx += 1;
                    }
                }
            }
        }
    }
    if idx < 8 {
        eprintln!("spike output parse failed; got {idx}/8 mem dumps. stderr: {stderr}\nstdout: {stdout}");
        return None;
    }
    Some(words)
}

// ---- Hardware runners (return final 8-word data window) ---------

fn run_single_hw(program: Vec<u32>, max_cycles: usize) -> [u32; 8] {
    // Our hardware has separate program/data ports; we use the
    // existing harness pattern.  Test programs encode SW
    // instructions with addresses 0x100..0x120 (matching where the
    // ELF window will be: 0x80001000..0x80001020 in Spike, but
    // index 0..8 in our data_mem).
    //
    // We use the LOW addresses (0..0x20) here in the test programs,
    // and translate to 0x80001000..0x80001020 when generating the
    // ELF for Spike (by adding lui x_base, 0x80001).  See the
    // spike-program builders below.
    let uut = Cpu::default();
    let mut data_mem: [u32; 256] = [0; 256];
    let mut reset_cycles_remaining = 2;
    let mut total_cycles: usize = 0;
    uut.run_fn(
        |out: SOut| {
            if reset_cycles_remaining > 0 { reset_cycles_remaining -= 1; return Some(ResetOrData::Reset); }
            if total_cycles >= max_cycles { return None; }
            total_cycles += 1;
            if out.mem_write {
                let addr_word = (out.mem_addr.raw() / 4) as usize;
                if addr_word < data_mem.len() { data_mem[addr_word] = out.mem_wdata.raw() as u32; }
            }
            let pc_word = (out.pc.raw() / 4) as usize;
            let instr = if pc_word < program.len() { program[pc_word] } else { 0 };
            let read_word = (out.mem_addr.raw() / 4) as usize;
            let mem_rdata = if read_word < data_mem.len() { data_mem[read_word] } else { 0 };
            Some(ResetOrData::Data(SInIn {
                instr: bits::<32>(instr as u128),
                mem_rdata: bits::<32>(mem_rdata as u128),
                int_pending: bits::<32>(0),
            }))
        },
        100,
    ).for_each(drop);
    let mut out = [0u32; 8];
    out.copy_from_slice(&data_mem[0..8]);
    out
}

fn run_pipelined_hw(program: Vec<u32>, max_cycles: usize) -> [u32; 8] {
    let uut = PipelinedCpu::default();
    let mut data_mem: [u32; 256] = [0; 256];
    let mut reset_cycles_remaining = 2;
    let mut total_cycles: usize = 0;
    uut.run_fn(
        |out: POut| {
            if reset_cycles_remaining > 0 { reset_cycles_remaining -= 1; return Some(ResetOrData::Reset); }
            if total_cycles >= max_cycles { return None; }
            total_cycles += 1;
            if out.mem_write {
                let addr_word = (out.mem_addr.raw() / 4) as usize;
                if addr_word < data_mem.len() { data_mem[addr_word] = out.mem_wdata.raw() as u32; }
            }
            let pc_word = (out.pc.raw() / 4) as usize;
            let instr = if pc_word < program.len() { program[pc_word] } else { 0 };
            let read_word = (out.mem_addr.raw() / 4) as usize;
            let mem_rdata = if read_word < data_mem.len() { data_mem[read_word] } else { 0 };
            Some(ResetOrData::Data(PIn {
                instr: bits::<32>(instr as u128),
                mem_rdata: bits::<32>(mem_rdata as u128),
                int_pending: bits::<32>(0),
            }))
        },
        100,
    ).for_each(drop);
    let mut out = [0u32; 8];
    out.copy_from_slice(&data_mem[0..8]);
    out
}

// ---- Encoding helpers (same shape as other test files) ----------

fn r_type(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    (funct7 & 0x7F) << 25 | (rs2 & 0x1F) << 20 | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12 | (rd & 0x1F) << 7 | (opcode & 0x7F)
}
fn i_type(imm: i32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    let imm_u = (imm as u32) & 0xFFF;
    (imm_u << 20) | (rs1 & 0x1F) << 15 | (funct3 & 0x7) << 12
        | (rd & 0x1F) << 7 | (opcode & 0x7F)
}
fn s_type(imm: i32, rs2: u32, rs1: u32, funct3: u32, opcode: u32) -> u32 {
    let imm_u = (imm as u32) & 0xFFF;
    let imm_high = (imm_u >> 5) & 0x7F;
    let imm_low = imm_u & 0x1F;
    (imm_high << 25) | (rs2 & 0x1F) << 20 | (rs1 & 0x1F) << 15
        | (funct3 & 0x7) << 12 | imm_low << 7 | (opcode & 0x7F)
}
fn lui(rd: u32, imm20: u32) -> u32 { (imm20 & 0xFFFFF) << 12 | (rd & 0x1F) << 7 | 0x37 }
fn u_type(imm: u32, rd: u32, opcode: u32) -> u32 {
    (imm & 0xFFFFF000) | (rd & 0x1F) << 7 | (opcode & 0x7F)
}
fn addi(rd: u32, rs1: u32, imm: i32) -> u32 { i_type(imm, rs1, 0, rd, 0x13) }
fn add(rd: u32, rs1: u32, rs2: u32) -> u32 { r_type(0, rs2, rs1, 0, rd, 0x33) }
fn sub(rd: u32, rs1: u32, rs2: u32) -> u32 { r_type(0b0100000, rs2, rs1, 0, rd, 0x33) }
fn xor(rd: u32, rs1: u32, rs2: u32) -> u32 { r_type(0, rs2, rs1, 4, rd, 0x33) }
fn and(rd: u32, rs1: u32, rs2: u32) -> u32 { r_type(0, rs2, rs1, 7, rd, 0x33) }
fn or_(rd: u32, rs1: u32, rs2: u32) -> u32 { r_type(0, rs2, rs1, 6, rd, 0x33) }
fn sll(rd: u32, rs1: u32, rs2: u32) -> u32 { r_type(0, rs2, rs1, 1, rd, 0x33) }
fn srl(rd: u32, rs1: u32, rs2: u32) -> u32 { r_type(0, rs2, rs1, 5, rd, 0x33) }
fn sra(rd: u32, rs1: u32, rs2: u32) -> u32 { r_type(0b0100000, rs2, rs1, 5, rd, 0x33) }
fn slt(rd: u32, rs1: u32, rs2: u32) -> u32 { r_type(0, rs2, rs1, 2, rd, 0x33) }
fn sltu(rd: u32, rs1: u32, rs2: u32) -> u32 { r_type(0, rs2, rs1, 3, rd, 0x33) }
fn sw(rs2: u32, rs1: u32, imm: i32) -> u32 { s_type(imm, rs2, rs1, 2, 0x23) }

const HALT: u32 = 0x0000_0063;  // beq x0, x0, +0

/// Build a "Spike" program: prepend a `lui x_base, 0x80001` so the
/// program writes its data window to 0x80001000+offset.  The
/// hardware sees the same instruction stream but the SW addresses
/// land in `data_mem[0]..data_mem[7]` because our hardware harness
/// indexes `data_mem` by `addr/4` which (for these high addresses)
/// truncates to wrong indices unless we adjust.
///
/// To keep both targets reading from the SAME effective address
/// space, we use a different construction: tests use SW to address
/// 0 and our hardware harness reads `data_mem[0]`; we then build a
/// **separate** Spike-targeted program that uses the same bytes as
/// instructions but with SW pointing at 0x80001000+offset, by
/// initializing a base register.
///
/// Common pattern:
///   x31 = base address (Spike: 0x80001000; hardware: 0)
///   ... program ...
/// We pass two variants of the program from each test:
/// `(spike_program, hw_program)` differing only in the base setup.
fn spike_program(body: Vec<u32>) -> Vec<u32> {
    let mut p = vec![
        lui(31, 0x80001),         // x31 = 0x80001000
    ];
    p.extend(body);
    p.push(HALT);
    p
}
fn hw_program(body: Vec<u32>) -> Vec<u32> {
    let mut p = vec![
        addi(31, 0, 0),           // x31 = 0 (hardware data_mem index 0)
    ];
    p.extend(body);
    p.push(HALT);
    p
}

/// Compare Spike's data window against both hardware cores'.
fn assert_spike_lockstep(label: &str, body: Vec<u32>, cycles: u32) {
    let Some(spike) = require_spike() else { return };
    let spike_words = match run_spike(&spike, &spike_program(body.clone()), cycles) {
        Some(w) => w,
        None => panic!("spike run failed for test {label}"),
    };
    let single_words = run_single_hw(hw_program(body.clone()), (cycles as usize + 4).max(20));
    let pipelined_words = run_pipelined_hw(hw_program(body.clone()), (cycles as usize + 4).max(20) * 3);
    assert_eq!(
        spike_words, single_words,
        "{label}: Spike ↔ single-cycle data-window mismatch\n  spike: {spike_words:#x?}\n  single: {single_words:#x?}",
    );
    assert_eq!(
        spike_words, pipelined_words,
        "{label}: Spike ↔ pipelined data-window mismatch\n  spike: {spike_words:#x?}\n  pipelined: {pipelined_words:#x?}",
    );
}

// ---- Tests --------------------------------------------------------

/// Sanity: verify Spike is reachable.  If it isn't, every test in
/// this file silently no-ops.
#[test]
fn spike_is_available() {
    let path = require_spike();
    if path.is_none() {
        eprintln!("(skipping all Spike tests — see require_spike message above)");
        return;
    }
    eprintln!("Spike at: {}", path.unwrap().display());
}

#[test]
fn spike_lockstep_basic_addi() {
    // x1 = 5; x2 = 10; x3 = x1 + x2 = 15; mem[base+0] = x3
    assert_spike_lockstep("addi+add+sw", vec![
        addi(1, 0, 5),
        addi(2, 0, 10),
        add(3, 1, 2),
        sw(3, 31, 0),         // mem[base+0] = 15
    ], 16);
}

#[test]
fn spike_lockstep_arith_chain() {
    // Chain of ALU operations writing successive results.
    assert_spike_lockstep("alu_chain", vec![
        addi(1, 0, 7),
        addi(2, 0, 3),
        add(3, 1, 2),         // x3 = 10
        sub(4, 1, 2),         // x4 = 4
        xor(5, 1, 2),         // x5 = 4
        and(6, 1, 2),         // x6 = 3
        or_(7, 1, 2),         // x7 = 7
        sw(3, 31, 0),
        sw(4, 31, 4),
        sw(5, 31, 8),
        sw(6, 31, 12),
        sw(7, 31, 16),
    ], 32);
}

#[test]
fn spike_lockstep_shifts() {
    // SLL / SRL / SRA — exercise shift semantics.
    assert_spike_lockstep("shifts", vec![
        addi(1, 0, 0x100),    // x1 = 0x100
        addi(2, 0, 4),        // x2 = 4
        sll(3, 1, 2),         // x3 = 0x1000
        srl(4, 1, 2),         // x4 = 0x10
        addi(5, 0, -1),       // x5 = 0xFFFFFFFF
        sra(6, 5, 2),         // x6 = 0xFFFFFFFF (arithmetic)
        srl(7, 5, 2),         // x7 = 0x3FFFFFFF (logical)
        sw(3, 31, 0),
        sw(4, 31, 4),
        sw(6, 31, 8),
        sw(7, 31, 12),
    ], 32);
}

#[test]
fn spike_lockstep_signed_compares() {
    // SLT / SLTU
    assert_spike_lockstep("slt_sltu", vec![
        addi(1, 0, 5),
        addi(2, 0, -3),       // x2 = 0xFFFFFFFD
        slt(3, 1, 2),         // signed:  5 < -3 = 0
        slt(4, 2, 1),         // signed: -3 <  5 = 1
        sltu(5, 1, 2),        // unsigned: 5 < 0xFFFFFFFD = 1
        sltu(6, 2, 1),        // unsigned: 0xFFFFFFFD < 5 = 0
        sw(3, 31, 0),
        sw(4, 31, 4),
        sw(5, 31, 8),
        sw(6, 31, 12),
    ], 32);
}

#[test]
fn spike_lockstep_negative_arithmetic() {
    // -1 + -1 = -2; -1 - -1 = 0
    assert_spike_lockstep("negative_arith", vec![
        addi(1, 0, -1),       // x1 = 0xFFFFFFFF
        addi(2, 0, -1),       // x2 = 0xFFFFFFFF
        add(3, 1, 2),         // x3 = 0xFFFFFFFE
        sub(4, 1, 2),         // x4 = 0
        sw(3, 31, 0),
        sw(4, 31, 4),
    ], 16);
}

#[test]
fn spike_lockstep_load_then_store() {
    // Store a value, load it back, store the loaded value.  Exercises
    // both the load path and the load/store ordering.
    assert_spike_lockstep("load_store", vec![
        addi(1, 0, 0xAB),
        sw(1, 31, 0),                 // mem[base+0] = 0xAB
        i_type(0, 31, 2, 2, 0x03),    // lw x2, 0(x31)
        sw(2, 31, 4),                 // mem[base+4] = 0xAB (loaded value)
    ], 32);
}

#[test]
fn spike_lockstep_lui_auipc() {
    // LUI puts an upper-immediate; AUIPC is PC + upper-immediate.
    // Just verify the constant LUI computation matches.
    assert_spike_lockstep("lui_auipc", vec![
        // x1 = LUI 0xABCDE → 0xABCDE000
        u_type(0xABCDE000, 1, 0x37),
        // x2 = AUIPC 0  → x2 = current PC (which is 0x80000008
        // because the lui_prefix at index 0 is one instr in front
        // of this body) — value depends on placement, just store it.
        u_type(0, 2, 0x17),
        addi(3, 1, 0),                // x3 = x1 = 0xABCDE000
        sw(1, 31, 0),                 // mem[+0] = LUI value
        sw(3, 31, 4),                 // mem[+4] = same
    ], 24);
}

/// b-type encoding helper.
fn beq(rs1: u32, rs2: u32, imm: i32) -> u32 {
    let imm_u = (imm as u32) & 0x1FFF;
    let bit12 = (imm_u >> 12) & 0x1;
    let bit11 = (imm_u >> 11) & 0x1;
    let b10_5 = (imm_u >> 5) & 0x3F;
    let b4_1 = (imm_u >> 1) & 0xF;
    (bit12 << 31) | (b10_5 << 25) | (rs2 & 0x1F) << 20 | (rs1 & 0x1F) << 15
        | (0 << 12) | (b4_1 << 8) | (bit11 << 7) | 0x63
}

#[test]
fn spike_lockstep_branch_taken() {
    // 0: addi x1 = 0xCC
    // 1: BEQ x0, x0, +8 → skip 1 instruction
    // 2: addi x1 = 0xDD       (SQUASHED)
    // 3: sw x1, 0(x31)        → store x1 (which is 0xCC, since the addi was squashed)
    assert_spike_lockstep("branch_taken", vec![
        addi(1, 0, 0xCC),
        beq(0, 0, 8),
        addi(1, 0, 0xDD),
        sw(1, 31, 0),
    ], 32);
}

#[test]
fn spike_lockstep_branch_not_taken() {
    // BNE x0, x0 is never taken; everything after executes normally.
    let bne = |rs1: u32, rs2: u32, imm: i32| -> u32 {
        let imm_u = (imm as u32) & 0x1FFF;
        let bit12 = (imm_u >> 12) & 0x1;
        let bit11 = (imm_u >> 11) & 0x1;
        let b10_5 = (imm_u >> 5) & 0x3F;
        let b4_1 = (imm_u >> 1) & 0xF;
        (bit12 << 31) | (b10_5 << 25) | (rs2 & 0x1F) << 20 | (rs1 & 0x1F) << 15
            | (1 << 12) | (b4_1 << 8) | (bit11 << 7) | 0x63
    };
    assert_spike_lockstep("branch_not_taken", vec![
        addi(1, 0, 0x11),
        bne(0, 0, 8),                       // never taken
        addi(1, 0, 0x22),                   // EXECUTES
        sw(1, 31, 0),                       // mem[+0] = 0x22
    ], 32);
}

#[test]
fn spike_lockstep_jal_skips_squashed_instr() {
    // JAL skips the next instruction; verify by storing x1 (whose
    // value depends on whether the squashed addi ran).  We do NOT
    // store the JAL link register because it contains the absolute
    // PC, which differs between Spike (entry 0x80000000) and our
    // hardware (entry 0).
    let jal = |rd: u32, imm: i32| -> u32 {
        let imm_u = (imm as u32) & 0x1F_FFFF;
        let bit20 = (imm_u >> 20) & 0x1;
        let b19_12 = (imm_u >> 12) & 0xFF;
        let bit11 = (imm_u >> 11) & 0x1;
        let b10_1 = (imm_u >> 1) & 0x3FF;
        (bit20 << 31) | (b19_12 << 12) | (bit11 << 20) | (b10_1 << 21)
            | (rd & 0x1F) << 7 | 0x6F
    };
    assert_spike_lockstep("jal_skip", vec![
        addi(1, 0, 0xAA),
        jal(5, 8),                          // jump +8, link to x5 (unobserved)
        addi(1, 0, 0xBB),                   // SQUASHED
        sw(1, 31, 0),                       // mem[+0] = 0xAA (proves squash worked)
    ], 32);
}

#[test]
fn spike_lockstep_random_seed_42_stress() {
    // A larger random-ish program — exercises multiple ALU types,
    // a branch, multiple stores.
    assert_spike_lockstep("random_42", vec![
        addi(1, 0, 0x100),
        addi(2, 0, 0x10),
        add(3, 1, 2),                       // 0x110
        sll(4, 2, 2),                       // 0x10 << 0x10 → arch shamt only uses low 5 bits → 0x10 << 16 = 0x100000
        sub(5, 3, 4),                       // depends
        xor(6, 1, 3),                       // 0x100 ^ 0x110 = 0x10
        or_(7, 2, 4),                       // 0x10 | 0x100000
        and(8, 3, 6),                       // 0x110 & 0x10 = 0x10
        slt(9, 4, 1),                       // signed: 0x100000 < 0x100 → 0
        sltu(10, 1, 4),                     // unsigned: 0x100 < 0x100000 → 1
        sw(3, 31, 0),
        sw(4, 31, 4),
        sw(5, 31, 8),
        sw(6, 31, 12),
        sw(7, 31, 16),
        sw(8, 31, 20),
        sw(9, 31, 24),
        sw(10, 31, 28),
    ], 64);
}
