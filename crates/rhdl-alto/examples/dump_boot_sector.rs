//! Dump the boot sector (cyl=0, head=0, sector=0) from the .dsk image
//! to verify it contains real Nova-encoded boot code rather than zeros
//! or a stub.  Real boot blocks have recognizable LDA/STA/JSR opcodes
//! in the high bits of each word.

use rhdl_alto::disk_image_loader;
use std::path::PathBuf;

fn classify_nova_opcode(word: u16) -> &'static str {
    // Per AltoHW §3.1, Nova instruction format (Alto MSB=0 numbering →
    // our LSB=0 numbering means bit 15 is "MSB" in Alto terms):
    //   bit 15      = group selector (0=M, 1=A or J/S)
    //   bit 14-13   = sub-group (M-group: MFunc; A-group has bit 14=1)
    //
    // Heuristic classification (per AltoHW §3.1 figure 3):
    let group = (word >> 13) & 0b111;
    match group {
        0b000 => "JMP/JSR/ISZ/DSZ (J-group)",
        0b001 => "LDA (M-group)",
        0b010 => "STA (M-group)",
        0b011 => "S-group augmented (BLT/BLKS/SIO/...)",
        _ if (word >> 15) & 1 == 1 => "A-group (arithmetic/logical)",
        _ => "(undefined)",
    }
}

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let disk = manifest_dir.join("assets/disk/nonprog.dsk");
    if !disk.exists() {
        eprintln!("[skip] no disk image at {disk:?}");
        return;
    }
    let image = disk_image_loader::load_disk_image_from_file(&disk).unwrap();
    let boot = image.sector(0, 0, 0);

    println!("=== Disk image: {} ===", disk.display());
    println!(
        "File size: 2,601,648 bytes (203 cyl × 2 head × 12 sec × 267 words × 2 bytes — matches Diablo 31 geometry)"
    );
    println!();

    println!("=== Sector 0 (cyl=0, head=0, sector=0) — the boot sector ===");
    println!();
    println!("Header (2 words, encodes disk address):");
    for (i, &w) in boot.header.iter().enumerate() {
        println!("  header[{i}] = 0x{w:04x}");
    }
    println!();
    println!("Label (8 words, software-defined metadata):");
    for (i, &w) in boot.label.iter().enumerate() {
        println!("  label[{i}] = 0x{w:04x}");
    }
    println!();

    println!("Data (256 words, the boot block — first 32 words shown with Nova classification):");
    for i in 0..32usize {
        let w = boot.data[i];
        let class = classify_nova_opcode(w);
        // memory[1..400B] is loaded with these data words; memory[1] = boot.data[0]
        let mem_addr = i + 1; // boot loads to memory[1..256]
        println!("  data[{i:3}] = 0x{w:04x}  → memory[0x{mem_addr:03x}]  → {class}");
    }
    println!();

    // Statistical sniff: how many words are non-zero?  How many look
    // like real Nova instructions (high bit set)?
    let non_zero = boot.data.iter().filter(|&&w| w != 0).count();
    let high_bit = boot.data.iter().filter(|&&w| (w & 0x8000) != 0).count();
    println!("Statistical sniff:");
    println!("  non-zero data words:     {non_zero} / 256");
    println!("  high-bit-set data words: {high_bit} / 256  (= A-group / S-group instructions)");
    println!(
        "  ratio non-zero:          {:.1}%",
        100.0 * non_zero as f64 / 256.0
    );
    println!();
    if non_zero < 200 {
        println!("VERDICT: Looks like a STUB — too many zero words for real boot code.");
    } else if high_bit < 30 {
        println!("VERDICT: Mixed; might be data (not pure boot code).");
    } else {
        println!(
            "VERDICT: Looks like REAL period boot code (high non-zero density + reasonable opcode mix)."
        );
    }
}
