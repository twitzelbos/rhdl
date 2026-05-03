use rhdl_alto::microcode_loader;
use rhdl_alto::isa::Microinstruction;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let rom_dir = manifest_dir.join("assets/rom");
    let microcode = microcode_loader::load_alto_ii_microcode_from_dir(&rom_dir).unwrap();
    let interesting = [0x12f, 0x147, 0x148, 0x149, 0x150, 0x151, 0x152, 0x153, 0x154, 0x130];
    for &mpc in &interesting {
        let word = microcode[mpc];
        let ui = Microinstruction::unpack(word);
        println!("MPC=0x{:03x}: word=0x{:08x}  rsel={}  alu={:?}  bs={:?}  f1={:?}  f2={:?}  t_load={}  l_load={}  next=0x{:03x}",
            mpc, word, ui.rsel.raw(), ui.aluf, ui.bs, ui.f1, ui.f2, ui.t_load, ui.l_load, ui.next.raw());
    }
}
