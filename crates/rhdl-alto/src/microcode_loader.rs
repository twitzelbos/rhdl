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
    "U55", "U64", "U65", "U63", "U53", "U60", "U61", "U62",
    // Bank 1
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
    Io { path: PathBuf, source: std::io::Error },
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
                write!(f, "PROM file {name} has wrong length: expected {PROM_BYTES} bytes, got {len}")
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
pub fn load_alto_ii_microcode(roms: &[&[u8]; NUM_MICROCODE_PROMS])
    -> Result<[u32; MICROCODE_WORDS], LoadError>
{
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
pub fn load_alto_ii_microcode_from_dir(dir: &Path)
    -> Result<[u32; MICROCODE_WORDS], LoadError>
{
    let mut bytes: Vec<Vec<u8>> = Vec::with_capacity(NUM_MICROCODE_PROMS);
    for name in PROM_FILENAMES.iter() {
        let path = dir.join(name);
        let data = std::fs::read(&path)
            .map_err(|e| LoadError::Io { path: path.clone(), source: e })?;
        bytes.push(data);
    }
    let refs: [&[u8]; NUM_MICROCODE_PROMS] =
        std::array::from_fn(|i| bytes[i].as_slice());
    load_alto_ii_microcode(&refs)
}

/// Decode a packed microcode-word array into [`Microinstruction`] values.
pub fn decode_microcode(words: &[u32; MICROCODE_WORDS])
    -> Vec<Microinstruction>
{
    words.iter().map(|&w| Microinstruction::unpack(w)).collect()
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
        let refs: [&[u8]; NUM_MICROCODE_PROMS] =
            std::array::from_fn(|i| roms_data[i].as_slice());
        let words = load_alto_ii_microcode(&refs).expect("load ok");
        assert_eq!(words[0], target_word, "bank-0 microaddress 0");
        // Other addresses should still apply MapWord on a zero
        // assembled word: 0 ^ 0xfff77bff = 0xfff77bff.
        assert_eq!(words[1], BIT_INVERSION_MASK, "bank-0 microaddress 1 (untouched, still XORed)");
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
        let refs: [&[u8]; NUM_MICROCODE_PROMS] =
            std::array::from_fn(|i| roms_data[i].as_slice());
        let words = load_alto_ii_microcode(&refs).expect("load ok");
        assert_eq!(words[0x400], target_word, "bank-1 microaddress 0");
    }

    #[test]
    fn wrong_rom_length_errors() {
        let mut roms_data: Vec<Vec<u8>> = (0..NUM_MICROCODE_PROMS)
            .map(|_| vec![0u8; PROM_BYTES])
            .collect();
        roms_data[3] = vec![0u8; 999];  // wrong size
        let refs: [&[u8]; NUM_MICROCODE_PROMS] =
            std::array::from_fn(|i| roms_data[i].as_slice());
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
        for &w in &[0u32, 0xffff_ffff, 0xdeadbeef, 0x12345678, BIT_INVERSION_MASK] {
            assert_eq!(map_word(w), w ^ BIT_INVERSION_MASK);
        }
    }

    #[test]
    fn address_map_inverts_bottom_10_bits() {
        assert_eq!(address_map_alto_ii(0), 0x3ff);
        assert_eq!(address_map_alto_ii(0x3ff), 0);
        assert_eq!(address_map_alto_ii(0x123), !0x123 & 0x3ff);
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
            .join("assets").join("rom");
        if !dir.join("U55").exists() {
            eprintln!("[real_proms_load_if_available] skipping — PROMs not present in {dir:?}");
            return;
        }
        let words = load_alto_ii_microcode_from_dir(&dir)
            .expect("load real PROMs");
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
        assert_eq!(mi.pack(), w0,
            "microinstruction at addr 0 must round-trip pack/unpack");
    }
}
