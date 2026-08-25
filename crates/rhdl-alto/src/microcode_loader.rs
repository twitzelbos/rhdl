//! Alto II microcode loader.
//!
//! The Alto II's microcode is stored across **16 PROM chips** (8 per
//! 1K-microinstruction bank, 2 banks for the full 2K).  Each PROM is
//! a 1024-byte SRAM-style dump where only the **low nibble** of each
//! byte is meaningful — eight chips OR'd together at staggered bit
//! positions assemble one 32-bit microinstruction.
//!
//! ## Three transforms applied during load
//!
//! 1. **PROM address-line inversion**: each chip is read at index
//!    `(!address) & 0x3ff` because the address lines on the Alto board
//!    are wired inverted.  See ContrAlto's `AddressMapAltoII`.
//! 2. **Nibble assembly**: each chip contributes its low 4 bits at a
//!    chip-specific bit position (0, 4, 8, ..., 28).  Eight chips
//!    fully cover all 32 bits.
//! 3. **Bit-inversion mask** `0xfff77bff`: every bit *except* bits 19,
//!    15, 10 is XORed.  Those three bits are the high bits of F1, F2,
//!    and the LoadL flag — already stored inverted in the PROM, so
//!    leaving them alone yields the right final value.
//!
//! Equivalent to: `final_word = assembled_word ^ 0xfff77bff`.
//!
//! ## ROM layout
//!
//! - Bank 0 (microaddress 0x000-0x3FF): `U55, U64, U65, U63, U53, U60, U61, U62`
//!   at bit positions 28, 24, 20, 16, 12, 8, 4, 0 respectively.
//! - Bank 1 (microaddress 0x400-0x7FF): `U54, U74, U75, U73, U52, U70, U71, U72`
//!   at bit positions 28, 24, 20, 16, 12, 8, 4, 0 respectively.
//!
//! ## Provenance and licensing
//!
//! The PROM dumps themselves are 1976 PARC firmware.  RHDL does **not**
//! commit them — they're fetched at test-time from the user's local
//! machine into `crates/rhdl-alto/assets/rom/` (gitignored).  The
//! reference dumps are mirrored in
//! [ContrAlto](https://github.com/livingcomputermuseum/ContrAlto/tree/master/Contralto/ROM/AltoII)
//! (AGPL-licensed; the bytes themselves are PARC firmware, but the
//! download is from an AGPL repository, so we don't redistribute).
//!
//! Reference parser: ContrAlto's
//! [`UCodeMemory.cs`](https://github.com/livingcomputermuseum/ContrAlto/blob/master/Contralto/CPU/UCodeMemory.cs)
//! `LoadAltoIIMicrocode` (lines ~269-320).

use crate::isa::Microinstruction;
use std::path::{Path, PathBuf};

/// Total microinstructions in the Alto II's two microcode banks.
pub const MICROCODE_WORDS: usize = 2048;

/// Per-chip 1024-byte PROM size.
pub const PROM_BYTES: usize = 1024;

/// Number of PROM chips required for both microcode banks.
pub const NUM_MICROCODE_PROMS: usize = 16;

/// Filenames of the 16 microcode PROM chip dumps, in load order.
/// First 8 cover bank 0 (microaddress 0x000-0x3FF); next 8 cover
/// bank 1 (microaddress 0x400-0x7FF).
pub const PROM_FILENAMES: [&str; NUM_MICROCODE_PROMS] = [
    // Bank 0
    "U55", "U64", "U65", "U63", "U53", "U60", "U61", "U62", // Bank 1
    "U54", "U74", "U75", "U73", "U52", "U70", "U71", "U72",
];

/// Bit position each PROM contributes its low nibble at.
const PROM_BIT_POSITIONS: [u32; NUM_MICROCODE_PROMS] = [
    28, 24, 20, 16, 12, 8, 4, 0, // Bank 0
    28, 24, 20, 16, 12, 8, 4, 0, // Bank 1
];

/// Bit-inversion mask applied to every assembled word.  Bits 19, 15,
/// and 10 are NOT inverted (already-inverted in the PROM).
const BIT_INVERSION_MASK: u32 = 0xfff77bff;

/// Errors that can occur while loading the microcode.
#[derive(Debug)]
pub enum LoadError {
    /// One of the PROM files wasn't readable.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// A PROM file wasn't the expected 1024 bytes long.
    WrongRomLength { name: String, len: usize },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io { path, source } => {
                write!(f, "failed to read PROM file {path:?}: {source}")
            }
            LoadError::WrongRomLength { name, len } => {
                write!(
                    f,
                    "PROM file {name} has wrong length: expected {PROM_BYTES} bytes, got {len}"
                )
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// Inversion of the PROM address lines on the Alto board.
fn address_map_alto_ii(address: usize) -> usize {
    (!address) & 0x3ff
}

/// Apply the bit-inversion correction (`MapWord` in ContrAlto).
fn map_word(word: u32) -> u32 {
    word ^ BIT_INVERSION_MASK
}

/// Load the Alto II microcode from 16 in-memory PROM byte arrays.
///
/// `roms` must be in the order documented by [`PROM_FILENAMES`].
pub fn load_alto_ii_microcode(
    roms: &[&[u8]; NUM_MICROCODE_PROMS],
) -> Result<[u32; MICROCODE_WORDS], LoadError> {
    let mut words = [0u32; MICROCODE_WORDS];

    for (rom_idx, &rom) in roms.iter().enumerate() {
        if rom.len() != PROM_BYTES {
            return Err(LoadError::WrongRomLength {
                name: PROM_FILENAMES[rom_idx].to_string(),
                len: rom.len(),
            });
        }
        // Bank 0 uses chips 0..8 at base 0x000; bank 1 uses chips 8..16 at base 0x400.
        let base = if rom_idx < 8 { 0x000 } else { 0x400 };
        let bit_pos = PROM_BIT_POSITIONS[rom_idx];
        for addr in 0..PROM_BYTES {
            let mapped = address_map_alto_ii(addr);
            let nibble = (rom[mapped] & 0x0f) as u32;
            words[base + addr] |= nibble << bit_pos;
        }
    }

    // MapWord pass: invert every bit except the three already-inverted ones.
    for w in words.iter_mut() {
        *w = map_word(*w);
    }

    Ok(words)
}

/// Load the Alto II microcode from a directory containing the 16
/// PROM files named [`PROM_FILENAMES`].
pub fn load_alto_ii_microcode_from_dir(dir: &Path) -> Result<[u32; MICROCODE_WORDS], LoadError> {
    let mut bytes: Vec<Vec<u8>> = Vec::with_capacity(NUM_MICROCODE_PROMS);
    for name in PROM_FILENAMES.iter() {
        let path = dir.join(name);
        let data = std::fs::read(&path).map_err(|e| LoadError::Io {
            path: path.clone(),
            source: e,
        })?;
        bytes.push(data);
    }
    let refs: [&[u8]; NUM_MICROCODE_PROMS] = std::array::from_fn(|i| bytes[i].as_slice());
    load_alto_ii_microcode(&refs)
}

/// Decode a packed microcode-word array into [`Microinstruction`] values.
pub fn decode_microcode(words: &[u32; MICROCODE_WORDS]) -> Vec<Microinstruction> {
    words.iter().map(|&w| Microinstruction::unpack(w)).collect()
}

// =====================================================================
// Constant ROM (C0..C3)
// =====================================================================
//
// The Alto's "Constant ROM" provides 256 16-bit constants used when
// the F1 = Constant code is asserted; the lookup index is composed of
// the RSEL (5 bits) + BS (3 bits) fields = 8 bits = 256 entries.  The
// ROM is stored across 4 PROM chips (C0, C1, C2, C3), each 256 bytes
// with low nibble per byte.  Bit positions: C0=12, C1=8, C2=4, C3=0.
//
// Three transforms applied during load (per ContrAlto's
// `ConstantMemory.cs` `LoadConstants`):
//
// 1. **Address scrambling** via `AddressMapConstantRom`:
//    bit i of input maps to bit `addressMapping[i]` of output, where
//    `addressMapping = [7, 2, 1, 0, 3, 4, 5, 6]`.
//    (Per "05a_AIM.pdf" PROM-pinout doc — addresses are wired in no
//    sane order.)
// 2. **Per-nibble bit reversal** via `DataMapConstantRom`:
//    reverse bits 0-3 (bit 0↔3, bit 1↔2).
// 3. **Final 16-bit inversion**: every constant word is inverted
//    (XORed with 0xffff) once all four chips have been OR'd in.
//
// AltoII does NOT additionally invert the input data byte
// (`flip=false`) — only AltoI does.

/// Number of constants in the Constant ROM.
pub const CONSTANT_ROM_WORDS: usize = 256;

/// Per-PROM size in bytes.
pub const CONSTANT_PROM_BYTES: usize = 256;

/// Number of PROMs that build the Constant ROM.
pub const NUM_CONSTANT_PROMS: usize = 4;

/// Filenames of the 4 Constant ROM PROMs in load order.
pub const CONSTANT_PROM_FILENAMES: [&str; NUM_CONSTANT_PROMS] = ["C0", "C1", "C2", "C3"];

/// Bit position each Constant ROM chip contributes its low nibble at.
const CONSTANT_PROM_BIT_POSITIONS: [u32; NUM_CONSTANT_PROMS] = [12, 8, 4, 0];

/// Address scramble table per "05a_AIM.pdf".
const CONSTANT_ROM_ADDRESS_MAP: [u32; 8] = [7, 2, 1, 0, 3, 4, 5, 6];

/// Apply the address-scramble transform.
// The index *is* the address being decoded; iterating the values
// would lose the thing the function returns.
#[allow(clippy::needless_range_loop)]
fn address_map_constant_rom(address: usize) -> usize {
    let mut mapped = 0usize;
    for i in 0..8 {
        if (address & (1 << i)) != 0 {
            mapped |= 1 << CONSTANT_ROM_ADDRESS_MAP[i] as usize;
        }
    }
    mapped
}

/// Reverse low 4 bits of a nibble.
fn data_map_constant_rom(data: u32) -> u32 {
    let mut mapped = 0u32;
    for i in 0..4 {
        if (data & (1 << i)) != 0 {
            mapped |= 1 << (3 - i);
        }
    }
    mapped
}

/// Load the Alto II Constant ROM from 4 in-memory PROM byte arrays.
///
/// `roms` must be in the order documented by [`CONSTANT_PROM_FILENAMES`].
// `addr` indexes two collections at once, so an iterator over one
// of them still needs the counter.
#[allow(clippy::needless_range_loop)]
pub fn load_alto_ii_constant_rom(
    roms: &[&[u8]; NUM_CONSTANT_PROMS],
) -> Result<[u16; CONSTANT_ROM_WORDS], LoadError> {
    let mut constants = [0u16; CONSTANT_ROM_WORDS];

    for (rom_idx, &rom) in roms.iter().enumerate() {
        if rom.len() != CONSTANT_PROM_BYTES {
            return Err(LoadError::WrongRomLength {
                name: CONSTANT_PROM_FILENAMES[rom_idx].to_string(),
                len: rom.len(),
            });
        }
        let bit_pos = CONSTANT_PROM_BIT_POSITIONS[rom_idx];
        for addr in 0..CONSTANT_PROM_BYTES {
            let mapped = address_map_constant_rom(addr);
            let nibble = data_map_constant_rom(rom[mapped] as u32 & 0xf);
            constants[addr] |= (nibble << bit_pos) as u16;
        }
    }

    // Final pass: invert every 16-bit word.
    for c in constants.iter_mut() {
        *c = !*c;
    }

    Ok(constants)
}

/// Load the Alto II Constant ROM from a directory containing the 4
/// PROM files.
pub fn load_alto_ii_constant_rom_from_dir(
    dir: &Path,
) -> Result<[u16; CONSTANT_ROM_WORDS], LoadError> {
    let mut bytes: Vec<Vec<u8>> = Vec::with_capacity(NUM_CONSTANT_PROMS);
    for name in CONSTANT_PROM_FILENAMES.iter() {
        let path = dir.join(name);
        let data = std::fs::read(&path).map_err(|e| LoadError::Io {
            path: path.clone(),
            source: e,
        })?;
        bytes.push(data);
    }
    let refs: [&[u8]; NUM_CONSTANT_PROMS] = std::array::from_fn(|i| bytes[i].as_slice());
    load_alto_ii_constant_rom(&refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct a synthetic PROM set that encodes a single known
    /// instruction word at microaddress 0 of bank 0, and verify the
    /// loader recovers exactly that word after both transforms.
    #[test]
    fn synthetic_single_instruction_round_trips() {
        // Pick a target word with a non-trivial bit pattern (contains
        // bits at every PROM position so address inversion + bit
        // inversion both have a visible effect).
        let target_word: u32 = 0xDEADBEEF;
        // The loader applies `word ^ 0xfff77bff`, so we have to put
        // the *pre-inversion* value into the PROMs.  pre_word = target ^ mask.
        let pre_word: u32 = target_word ^ BIT_INVERSION_MASK;

        // Build the 16 PROMs.  For bank 0 (chips 0..8), at PROM
        // address `address_map_alto_ii(0) = 0x3ff`, write the nibble
        // each chip contributes; everything else stays zero.
        let mut roms_data: Vec<Vec<u8>> = (0..NUM_MICROCODE_PROMS)
            .map(|_| vec![0u8; PROM_BYTES])
            .collect();
        for chip_idx in 0..8 {
            let bit_pos = PROM_BIT_POSITIONS[chip_idx];
            let nibble = (pre_word >> bit_pos) & 0xf;
            // microaddress 0 → PROM index 0x3ff after inversion
            roms_data[chip_idx][address_map_alto_ii(0)] = nibble as u8;
        }
        let refs: [&[u8]; NUM_MICROCODE_PROMS] = std::array::from_fn(|i| roms_data[i].as_slice());
        let words = load_alto_ii_microcode(&refs).expect("load ok");
        assert_eq!(words[0], target_word, "bank-0 microaddress 0");
        // Other addresses should still apply MapWord on a zero
        // assembled word: 0 ^ 0xfff77bff = 0xfff77bff.
        assert_eq!(
            words[1], BIT_INVERSION_MASK,
            "bank-0 microaddress 1 (untouched, still XORed)"
        );
    }

    #[test]
    fn synthetic_bank_1_round_trip() {
        // Same idea, but place the word at bank 1 microaddress 0
        // (which lives at index 0x400 in the assembled array).
        let target_word: u32 = 0x12345678;
        let pre_word: u32 = target_word ^ BIT_INVERSION_MASK;
        let mut roms_data: Vec<Vec<u8>> = (0..NUM_MICROCODE_PROMS)
            .map(|_| vec![0u8; PROM_BYTES])
            .collect();
        for chip_idx in 8..16 {
            let bit_pos = PROM_BIT_POSITIONS[chip_idx];
            let nibble = (pre_word >> bit_pos) & 0xf;
            roms_data[chip_idx][address_map_alto_ii(0)] = nibble as u8;
        }
        let refs: [&[u8]; NUM_MICROCODE_PROMS] = std::array::from_fn(|i| roms_data[i].as_slice());
        let words = load_alto_ii_microcode(&refs).expect("load ok");
        assert_eq!(words[0x400], target_word, "bank-1 microaddress 0");
    }

    #[test]
    fn wrong_rom_length_errors() {
        let mut roms_data: Vec<Vec<u8>> = (0..NUM_MICROCODE_PROMS)
            .map(|_| vec![0u8; PROM_BYTES])
            .collect();
        roms_data[3] = vec![0u8; 999]; // wrong size
        let refs: [&[u8]; NUM_MICROCODE_PROMS] = std::array::from_fn(|i| roms_data[i].as_slice());
        let err = load_alto_ii_microcode(&refs).unwrap_err();
        match err {
            LoadError::WrongRomLength { name, len } => {
                assert_eq!(name, "U63");
                assert_eq!(len, 999);
            }
            _ => panic!("expected WrongRomLength error"),
        }
    }

    #[test]
    fn map_word_is_xor_with_inversion_mask() {
        // Sanity-check the simplification.
        for &w in &[
            0u32,
            0xffff_ffff,
            0xdeadbeef,
            0x12345678,
            BIT_INVERSION_MASK,
        ] {
            assert_eq!(map_word(w), w ^ BIT_INVERSION_MASK);
        }
    }

    #[test]
    fn address_map_inverts_bottom_10_bits() {
        assert_eq!(address_map_alto_ii(0), 0x3ff);
        assert_eq!(address_map_alto_ii(0x3ff), 0);
        assert_eq!(address_map_alto_ii(0x123), !0x123 & 0x3ff);
    }

    // ---- Constant ROM tests ---------------------------------------

    #[test]
    fn address_map_constant_rom_is_self_inverse_under_table() {
        // Spot-check: addressMapping = [7,2,1,0,3,4,5,6].
        // bit 0 → bit 7, so address 0x01 → 0x80.
        assert_eq!(address_map_constant_rom(0x01), 0x80);
        // bit 3 → bit 0, so address 0x08 → 0x01.
        assert_eq!(address_map_constant_rom(0x08), 0x01);
        // address 0 → 0
        assert_eq!(address_map_constant_rom(0x00), 0x00);
        // all bits set → all bits set (since the map is a permutation)
        assert_eq!(address_map_constant_rom(0xff), 0xff);
    }

    #[test]
    fn data_map_constant_rom_reverses_low_4_bits() {
        // bit 0 → bit 3
        assert_eq!(data_map_constant_rom(0x1), 0x8);
        // bit 1 → bit 2
        assert_eq!(data_map_constant_rom(0x2), 0x4);
        // bit 3 → bit 0
        assert_eq!(data_map_constant_rom(0x8), 0x1);
        // 0xf → 0xf (palindrome)
        assert_eq!(data_map_constant_rom(0xf), 0xf);
    }

    #[test]
    fn constant_rom_synthetic_round_trip() {
        // Build a synthetic constant ROM that places a known target
        // word at output index 0.  Working backwards through the
        // three transforms:
        //   final[i] = !(C0[map(i)]<<12 | C1[map(i)]<<8 | C2[map(i)]<<4 | C3[map(i)])
        // For final[0] = 0xCAFE, we need:
        //   pre_invert = !0xCAFE & 0xffff = 0x3501
        //   so the OR'd nibbles must be 0x3501
        //   C0_nibble (at pos 12) = 0x3 → reversed = 0xC → C0[map(0)] = 0xC
        //   C1_nibble (at pos 8)  = 0x5 → reversed = 0xA → C1[map(0)] = 0xA
        //   C2_nibble (at pos 4)  = 0x0 → reversed = 0x0 → C2[map(0)] = 0x0
        //   C3_nibble (at pos 0)  = 0x1 → reversed = 0x8 → C3[map(0)] = 0x8
        // map(0) = 0, so put bytes at index 0 of each PROM.
        let mut roms_data: Vec<Vec<u8>> = (0..NUM_CONSTANT_PROMS)
            .map(|_| vec![0u8; CONSTANT_PROM_BYTES])
            .collect();
        roms_data[0][0] = 0xC; // C0
        roms_data[1][0] = 0xA; // C1
        roms_data[2][0] = 0x0; // C2
        roms_data[3][0] = 0x8; // C3
        let refs: [&[u8]; NUM_CONSTANT_PROMS] = std::array::from_fn(|i| roms_data[i].as_slice());
        let constants = load_alto_ii_constant_rom(&refs).expect("load ok");
        assert_eq!(
            constants[0], 0xCAFE,
            "constant[0] should round-trip through scramble + reverse + invert"
        );
        // Untouched indices: all-zero PROMs → final = !(0) = 0xffff.
        assert_eq!(constants[1], 0xffff);
    }

    #[test]
    fn real_constant_rom_loads_if_available() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("rom");
        if !dir.join("C0").exists() {
            eprintln!(
                "[real_constant_rom_loads_if_available] skipping — Constant ROMs not present in {dir:?}"
            );
            return;
        }
        let constants = load_alto_ii_constant_rom_from_dir(&dir).expect("load real Constant ROMs");
        assert_eq!(constants.len(), CONSTANT_ROM_WORDS);
        // Per the Alto ucode, constant index 0 ought to be 0 (the
        // microcode uses a known "constant zero" lookup).  We can't
        // confirm this without disassembling the ucode but we can
        // verify the loader produces *some* non-trivial pattern
        // (not all-zeros, not all-ones).
        let all_zero = constants.iter().all(|&c| c == 0);
        let all_ones = constants.iter().all(|&c| c == 0xffff);
        assert!(
            !all_zero && !all_ones,
            "Constant ROM should contain a meaningful pattern, not {} all of {:04x}",
            if all_zero { "all-zero" } else { "all-one" },
            constants[0]
        );
    }

    /// Real-PROM integration: only runs if the actual ROM dumps are
    /// available in `assets/rom/`.  Skipped otherwise so CI doesn't
    /// fail on machines without the (unredistributable) PROM bytes.
    ///
    /// To enable: place the 16 PROM files (`U52`, `U53`, ..., `U75`)
    /// in `crates/rhdl-alto/assets/rom/` and re-run.
    #[test]
    fn real_proms_load_if_available() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("rom");
        if !dir.join("U55").exists() {
            eprintln!("[real_proms_load_if_available] skipping — PROMs not present in {dir:?}");
            return;
        }
        let words = load_alto_ii_microcode_from_dir(&dir).expect("load real PROMs");
        assert_eq!(words.len(), MICROCODE_WORDS);

        // Spot-check microaddress 0 — the Alto's "Silent Boot" entry
        // point per the boot-block disassembly.  This MUST decode
        // to a valid microinstruction; we can't predict its exact
        // bit pattern without ContrAlto running but we can verify
        // the loader didn't produce all-ones or all-zeros.
        let w0 = words[0];
        assert_ne!(w0, 0u32, "microaddress 0 must not be all-zeros");
        assert_ne!(w0, 0xffff_ffff, "microaddress 0 must not be all-ones");

        // The decoded microinstruction must be well-formed (round-trips
        // through pack/unpack).
        let mi = Microinstruction::unpack(w0);
        assert_eq!(
            mi.pack(),
            w0,
            "microinstruction at addr 0 must round-trip pack/unpack"
        );
    }
}
