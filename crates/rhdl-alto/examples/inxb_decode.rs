use rhdl_alto::microcode_loader;
use rhdl_alto::isa::Microinstruction;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let rom_dir = manifest_dir.join("assets/rom");
    let microcode = microcode_loader::load_alto_ii_microcode_from_dir(&rom_dir).unwrap();
    let interesting = [0x121, 0x129, 0x13a, 0x13b, 0x13c, 0x13d, 0x13e, 0x13f, 0x138, 0x139, 0x140, 0x141];
    for &mpc in &interesting {
        let word = microcode[mpc];
        let ui = Microinstruction::unpack(word);
        println!("MPC=0x{:03x}: word=0x{:08x}  rsel={}  alu={:?}  bs={:?}  f1={:?}  f2={:?}  t_load={}  l_load={}  next=0x{:03x}",
            mpc, word, ui.rsel.raw(), ui.aluf, ui.bs, ui.f1, ui.f2, ui.t_load, ui.l_load, ui.next.raw());
    }

    // Dump constant ROM values relevant to KSEC's disk-controller writes:
    //   - constant_rom[(rsel<<3) | bs] is the value used when F1=Constant
    //     or F2=Constant overrides BUS.
    //   - At KSEC's MPC=0x382 (writes KCOM), rsel=5, bs=1 → index = 41.
    //   - At KSEC's MPC=0x37c (writes KCWA), rsel=0, bs=0 → index = 0.
    //   - At KSEC's MPC=0x37d (issues StoreMd), rsel=0, bs=4 → index = 4.
    let crom = microcode_loader::load_alto_ii_constant_rom_from_dir(&rom_dir).unwrap();
    println!();
    println!("=== Constant ROM values relevant to KSEC dispatch ===");
    let probe_indices = [
        (0,  "0x37c WriteKcwa: rsel=0,bs=0"),
        (4,  "0x37d StoreMd: rsel=0,bs=4 (TaskSpec4 = ←KDATA in disk)"),
        (41, "0x382 WriteKcomm: rsel=5,bs=1 → KCOM value (bit15=transfer_request)"),
        (113, "0x383 LoadMar: rsel=14,bs=1"),
        (59, "0x13b LoadMar: rsel=7,bs=3 → MAR target (BS+F2=Constant overrides)"),
    ];
    for &(idx, label) in &probe_indices {
        let v = crom[idx];
        println!("  constant_rom[{:>3}] = 0x{:04x} ({}) — {}",
            idx, v, format!("bit15={}", (v >> 15) & 1), label);
    }
}
