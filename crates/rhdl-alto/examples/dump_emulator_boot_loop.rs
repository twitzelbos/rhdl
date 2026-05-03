//! Dump the Emulator's boot-fetch loop microcode (MPC 0x130-0x156) to
//! understand exactly what each cycle reads/writes and where the
//! "Nova bootstrap" comes from.

use rhdl_alto::isa::Microinstruction;
use rhdl_alto::microcode_loader;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from("crates/rhdl-alto/assets/rom");
    if !dir.join("U55").exists() {
        eprintln!("[skip] PROM assets missing");
        return;
    }
    let microcode = microcode_loader::load_alto_ii_microcode_from_dir(&dir).unwrap();

    println!("=== Emulator boot fetch loop (MPC 0x130-0x156) ===");
    for mpc in [
        0x000u16, // NOVEM (Emulator reset entry)
        0x152, 0x153, 0x154, // boot dance
        0x130, 0x14e, // Emulator entry
        0x150, 0x151, // PC fetch + load IR
        0x155, 0x156, // post-fetch dispatch
    ] {
        let raw = microcode[mpc as usize];
        let mi = Microinstruction::unpack(raw);
        println!(
            "MPC=0x{mpc:03x} raw=0x{raw:08x} rsel={:>2} aluf={:?} bs={:?} f1={:?} f2={:?} t={} l={} next=0x{:03x}",
            mi.rsel.raw(), mi.aluf, mi.bs, mi.f1, mi.f2,
            mi.t_load as u8, mi.l_load as u8, mi.next.raw(),
        );
    }
}
