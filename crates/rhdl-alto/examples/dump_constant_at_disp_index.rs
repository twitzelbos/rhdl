//! Dump Constant ROM[RSEL, BS] at the indices used by the wait-loop
//! microinstructions, to confirm the BS>=4 AND-masking hypothesis.
//!
//! Per spec §2.2: "The constant memory is gated to the bus by F1=7,
//! F2=7, or BS>=4. ... This works because the processor bus ANDs if
//! more than one source is gated to it."
//!
//! Our impl currently only handles F1=Constant / F2=Constant.  The
//! BS>=4 path is missing.  This dump confirms the masks that would
//! apply at the divergence point.

use rhdl_alto::microcode_loader;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from("crates/rhdl-alto/assets/rom");
    if !dir.join("C0").exists() {
        eprintln!("[skip] constant ROM PROM assets missing");
        return;
    }
    let constants = microcode_loader::load_alto_ii_constant_rom_from_dir(&dir).unwrap();

    println!("=== Constant ROM masks at BS>=4 indices used by boot dance ===");
    println!("Index = (RSEL << 3) | BS");
    println!();
    println!("MPC=0x153 (BS=InstructionRegister=7, RSEL=0):");
    let idx = (0 << 3) | 7;
    println!(
        "  constants[{idx}] = 0x{:04x}  (mask AND'd with DISP value)",
        constants[idx]
    );
    println!();
    println!("MPC=0x150 (BS=ReadR=0, RSEL=5): BS<4, no mask applies (just for reference)");
    println!();
    println!("Dump of ALL BS>=4 mask constants (BS=4,5,6,7 across all RSEL=0..31):");
    for bs in 4..=7u8 {
        println!("  BS={bs}:");
        for rsel in 0..32u8 {
            let i = ((rsel as usize) << 3) | (bs as usize);
            let v = constants[i];
            if v != 0xFFFF {
                println!("    RSEL={rsel:>2} → constants[{i:>3}] = 0x{:04x}", v);
            }
        }
    }
    println!();
    println!("(Indices showing 0xFFFF were omitted — those are no-op masks.)");
}
