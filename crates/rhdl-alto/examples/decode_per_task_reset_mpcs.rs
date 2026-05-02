//! Decode the real Alto microcode at the per-task reset MPCs (0..15)
//! to understand what each task starts executing.  Useful for figuring
//! out which F1/F2 codes need real semantics for the boot path to
//! advance.

use rhdl_alto::isa::Microinstruction;
use rhdl_alto::microcode_loader;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from("crates/rhdl-alto/assets/rom");
    if !dir.join("U55").exists() {
        eprintln!("[skip] PROM assets missing under {dir:?}");
        return;
    }
    let microcode = microcode_loader::load_alto_ii_microcode_from_dir(&dir).unwrap();

    println!("Per-task reset MPCs (= task number per Alto Hardware Manual §2):\n");
    let task_names = [
        "Emulator", "(unused)", "(unused)", "(unused)", "Disk Sector",
        "(unused)", "(unused)", "Ethernet", "MRT", "Display Word",
        "Cursor", "Display Horizontal", "Display Vertical", "Parity",
        "Disk Word", "(unused)",
    ];
    for k in 0..16u32 {
        let raw = microcode[k as usize];
        let mi = Microinstruction::unpack(raw);
        println!(
            "Task {k:2} ({:>20}) MPC={k:#05x} instr=0x{raw:08x}",
            task_names[k as usize]
        );
        println!(
            "  rsel={:>2} aluf={:?} bs={:?} f1={:?} f2={:?} t={} l={} next={:#05x}",
            mi.rsel.raw(),
            mi.aluf,
            mi.bs,
            mi.f1,
            mi.f2,
            mi.t_load,
            mi.l_load,
            mi.next.raw()
        );
    }
}
