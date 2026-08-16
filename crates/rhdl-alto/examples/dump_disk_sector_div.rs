//! Dump microinstructions around MPC=0x38c (the Disk Sector
//! divergence found at lockstep matched=17).  Determines which
//! F2/F1 is responsible for the off-by-one NEXT.

use rhdl_alto::isa::Microinstruction;
use rhdl_alto::microcode_loader;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from("crates/rhdl-alto/assets/rom");
    if !dir.join("U55").exists() {
        return;
    }
    let microcode = microcode_loader::load_alto_ii_microcode_from_dir(&dir).unwrap();

    println!("=== Around MPC=0x38c (Disk Sector divergence) ===");
    for mpc in 0x388..=0x390u16 {
        let raw = microcode[mpc as usize];
        let mi = Microinstruction::unpack(raw);
        println!(
            "MPC=0x{mpc:03x} raw=0x{raw:08x} rsel={:>2} aluf={:?} bs={:?} f1={:?} f2={:?} t={} l={} next=0x{:03x}",
            mi.rsel.raw(),
            mi.aluf,
            mi.bs,
            mi.f1,
            mi.f2,
            mi.t_load as u8,
            mi.l_load as u8,
            mi.next.raw(),
        );
    }
    println!("\n=== Predecessors that point to 0x38c or 0x38d ===");
    for (mpc_idx, &raw) in microcode.iter().enumerate() {
        let mi = Microinstruction::unpack(raw);
        let n = mi.next.raw() as u16;
        if (n & !1) == 0x38c {
            println!(
                "MPC=0x{mpc_idx:03x} → next=0x{n:03x}  rsel={:>2} aluf={:?} bs={:?} f1={:?} f2={:?} t={} l={}",
                mi.rsel.raw(),
                mi.aluf,
                mi.bs,
                mi.f1,
                mi.f2,
                mi.t_load as u8,
                mi.l_load as u8,
            );
        }
    }
}
