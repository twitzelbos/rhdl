use rhdl_alto::microcode_loader;
use rhdl_alto::isa::Microinstruction;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let rom_dir = manifest_dir.join("assets/rom");
    let microcode = microcode_loader::load_alto_ii_microcode_from_dir(&rom_dir).unwrap();
    let interesting = [0x000, 0x152, 0x153, 0x154, 0x004, 0x37c, 0x37d, 0x381, 0x382, 0x383, 0x384, 0x385, 0x386, 0x387, 0x388, 0x389, 0x38a, 0x38b, 0x38c];
    for &mpc in &interesting {
        let word = microcode[mpc];
        let ui = Microinstruction::unpack(word);
        println!("MPC=0x{:03x}: word=0x{:08x}  rsel={}  alu={:?}  bs={:?}  f1={:?}  f2={:?}  t_load={}  l_load={}  next=0x{:03x}",
            mpc, word, ui.rsel.raw(), ui.aluf, ui.bs, ui.f1, ui.f2, ui.t_load, ui.l_load, ui.next.raw());
    }
}
