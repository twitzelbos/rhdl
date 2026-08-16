//! Alto Diablo-31 disk-image loader (`.dsk` format).
//!
//! The `.dsk` files distributed with ContrAlto and on Bitsavers store a
//! Diablo-31 disk pack as a flat byte stream:
//!
//! - 203 cylinders × 2 heads × 12 sectors = 4872 sectors
//! - 534 bytes per sector
//! - **2,601,648 bytes total**
//!
//! ## Per-sector layout (in stream order)
//!
//! | Offset | Size      | Field   |
//! |-------:|----------:|---------|
//! |    0   | 2 bytes   | Pad (Bitsavers' "extra word")  |
//! |    2   | 4 bytes   | Header (2 16-bit words)        |
//! |    6   | 16 bytes  | Label  (8 16-bit words)        |
//! |   22   | 512 bytes | Data   (256 16-bit words)      |
//!
//! All 16-bit words are **little-endian** (low byte first).
//!
//! ## Sector ordering on the file
//!
//! Sectors appear in the file in the order:
//!
//! ```text
//! for cylinder in 0..203 {
//!     for head in 0..2 {
//!         for sector in 0..12 {
//!             /* 534 bytes */
//!         }
//!     }
//! }
//! ```
//!
//! ## Provenance and licensing
//!
//! The `.dsk` files themselves originate from CHM / Bitsavers archives;
//! ContrAlto mirrors them.  RHDL does **not** redistribute them.
//!
//! Reference parser: ContrAlto's
//! [`DiskPack.cs`](https://github.com/livingcomputermuseum/ContrAlto/blob/master/Contralto/IO/DiskPack.cs)
//! `DiskSector` constructor (lines ~107-148), `GetUShortArray` (lines
//! ~228-242).

use std::path::{Path, PathBuf};

/// Diablo-31 geometry constants.
pub const CYLINDERS: usize = 203;
pub const HEADS: usize = 2;
pub const SECTORS_PER_TRACK: usize = 12;
pub const TOTAL_SECTORS: usize = CYLINDERS * HEADS * SECTORS_PER_TRACK;

/// Word counts within a sector.
pub const HEADER_WORDS: usize = 2;
pub const LABEL_WORDS: usize = 8;
pub const DATA_WORDS: usize = 256;

/// Byte counts within a sector (Bitsavers' 2-byte pad first).
pub const PAD_BYTES: usize = 2;
pub const HEADER_BYTES: usize = HEADER_WORDS * 2;
pub const LABEL_BYTES: usize = LABEL_WORDS * 2;
pub const DATA_BYTES: usize = DATA_WORDS * 2;
pub const SECTOR_BYTES: usize = PAD_BYTES + HEADER_BYTES + LABEL_BYTES + DATA_BYTES;

/// Total `.dsk` file size for a Diablo-31 image: 2,601,648 bytes.
pub const FILE_SIZE_BYTES: usize = TOTAL_SECTORS * SECTOR_BYTES;

/// One sector's contents, parsed from the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskSector {
    /// Cylinder this sector lives on (0..CYLINDERS).
    pub cylinder: u16,
    /// Head this sector lives on (0..HEADS).
    pub head: u16,
    /// Sector index within the track (0..SECTORS_PER_TRACK).
    pub sector: u16,
    /// 2-word header.
    pub header: [u16; HEADER_WORDS],
    /// 8-word label (file metadata, next-block pointer).
    pub label: [u16; LABEL_WORDS],
    /// 256-word data payload.
    pub data: [u16; DATA_WORDS],
}

/// A complete loaded `.dsk` image.
#[derive(Clone, Debug)]
pub struct DiskImage {
    /// Sectors in `[cylinder][head][sector]` order, flattened to a Vec
    /// for simple linear iteration.  Length = `TOTAL_SECTORS`.
    pub sectors: Vec<DiskSector>,
}

impl DiskImage {
    /// Returns the sector at `(cylinder, head, sector)` or panics if
    /// any index is out of range.
    pub fn sector(&self, cylinder: usize, head: usize, sector: usize) -> &DiskSector {
        let idx = cylinder * HEADS * SECTORS_PER_TRACK + head * SECTORS_PER_TRACK + sector;
        &self.sectors[idx]
    }
}

/// Errors that can arise loading a `.dsk` file.
#[derive(Debug)]
pub enum LoadError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// File size doesn't match the expected `FILE_SIZE_BYTES`.
    WrongFileSize { expected: usize, actual: usize },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io { path, source } => {
                write!(f, "failed to read disk image {path:?}: {source}")
            }
            LoadError::WrongFileSize { expected, actual } => {
                write!(
                    f,
                    "wrong .dsk file size: expected {expected} bytes (Diablo 31), got {actual}"
                )
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// Read a little-endian 16-bit word at byte offset `off`.
fn le_u16(bytes: &[u8], off: usize) -> u16 {
    (bytes[off] as u16) | ((bytes[off + 1] as u16) << 8)
}

/// Parse the in-memory bytes of a `.dsk` file into a [`DiskImage`].
pub fn parse_disk_image(bytes: &[u8]) -> Result<DiskImage, LoadError> {
    if bytes.len() != FILE_SIZE_BYTES {
        return Err(LoadError::WrongFileSize {
            expected: FILE_SIZE_BYTES,
            actual: bytes.len(),
        });
    }

    let mut sectors: Vec<DiskSector> = Vec::with_capacity(TOTAL_SECTORS);
    let mut off = 0usize;

    for cylinder in 0..CYLINDERS {
        for head in 0..HEADS {
            for sector in 0..SECTORS_PER_TRACK {
                // Skip the 2-byte pad.
                let mut p = off + PAD_BYTES;

                let mut header = [0u16; HEADER_WORDS];
                for h in &mut header {
                    *h = le_u16(bytes, p);
                    p += 2;
                }
                let mut label = [0u16; LABEL_WORDS];
                for l in &mut label {
                    *l = le_u16(bytes, p);
                    p += 2;
                }
                let mut data = [0u16; DATA_WORDS];
                for d in &mut data {
                    *d = le_u16(bytes, p);
                    p += 2;
                }

                sectors.push(DiskSector {
                    cylinder: cylinder as u16,
                    head: head as u16,
                    sector: sector as u16,
                    header,
                    label,
                    data,
                });

                off += SECTOR_BYTES;
            }
        }
    }

    Ok(DiskImage { sectors })
}

/// Load a `.dsk` file from disk.
pub fn load_disk_image_from_file(path: &Path) -> Result<DiskImage, LoadError> {
    let bytes = std::fs::read(path).map_err(|e| LoadError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    parse_disk_image(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an in-memory synthetic `.dsk` with a unique signature in
    /// each sector and verify it round-trips through the loader.
    #[test]
    fn synthetic_round_trip() {
        let mut bytes = vec![0u8; FILE_SIZE_BYTES];
        for s in 0..TOTAL_SECTORS {
            let off = s * SECTOR_BYTES;
            // Pad bytes set to 0xFF so we can verify they're skipped.
            bytes[off] = 0xFF;
            bytes[off + 1] = 0xFF;
            // Header[0] = sector index; Header[1] = sector_index XOR 0xa5a5
            let h0 = (s & 0xffff) as u16;
            let h1 = h0 ^ 0xa5a5;
            bytes[off + PAD_BYTES + 0] = (h0 & 0xff) as u8;
            bytes[off + PAD_BYTES + 1] = ((h0 >> 8) & 0xff) as u8;
            bytes[off + PAD_BYTES + 2] = (h1 & 0xff) as u8;
            bytes[off + PAD_BYTES + 3] = ((h1 >> 8) & 0xff) as u8;
            // Label[0] = 0xdead
            bytes[off + PAD_BYTES + HEADER_BYTES + 0] = 0xad;
            bytes[off + PAD_BYTES + HEADER_BYTES + 1] = 0xde;
            // Data[0..3] = sector index repeated
            for d in 0..4usize {
                let p = off + PAD_BYTES + HEADER_BYTES + LABEL_BYTES + d * 2;
                bytes[p] = (h0 & 0xff) as u8;
                bytes[p + 1] = ((h0 >> 8) & 0xff) as u8;
            }
        }

        let img = parse_disk_image(&bytes).expect("parse ok");
        assert_eq!(img.sectors.len(), TOTAL_SECTORS);

        for s in 0..TOTAL_SECTORS {
            let h0 = (s & 0xffff) as u16;
            assert_eq!(
                img.sectors[s].header[0], h0,
                "sector {s} header[0] mismatch"
            );
            assert_eq!(
                img.sectors[s].header[1],
                h0 ^ 0xa5a5,
                "sector {s} header[1] mismatch"
            );
            assert_eq!(
                img.sectors[s].label[0], 0xdead,
                "sector {s} label[0] mismatch"
            );
            for d in 0..4 {
                assert_eq!(img.sectors[s].data[d], h0, "sector {s} data[{d}] mismatch");
            }
        }
    }

    #[test]
    fn wrong_file_size_errors() {
        let bytes = vec![0u8; 1234];
        let err = parse_disk_image(&bytes).unwrap_err();
        match err {
            LoadError::WrongFileSize { expected, actual } => {
                assert_eq!(expected, FILE_SIZE_BYTES);
                assert_eq!(actual, 1234);
            }
            _ => panic!("expected WrongFileSize"),
        }
    }

    #[test]
    fn cylinder_head_sector_indexing() {
        let mut bytes = vec![0u8; FILE_SIZE_BYTES];
        // Mark sector (5, 1, 7) data[0] with a recognizable pattern.
        let s_idx = 5 * HEADS * SECTORS_PER_TRACK + 1 * SECTORS_PER_TRACK + 7;
        let off = s_idx * SECTOR_BYTES + PAD_BYTES + HEADER_BYTES + LABEL_BYTES;
        bytes[off] = 0xCD;
        bytes[off + 1] = 0xAB;
        let img = parse_disk_image(&bytes).expect("parse ok");
        assert_eq!(img.sector(5, 1, 7).data[0], 0xABCD);
        assert_eq!(img.sector(5, 1, 7).cylinder, 5);
        assert_eq!(img.sector(5, 1, 7).head, 1);
        assert_eq!(img.sector(5, 1, 7).sector, 7);
    }

    /// Real-disk integration: only runs if `nonprog.dsk` is in
    /// `assets/disk/`.  Skipped otherwise.
    #[test]
    fn real_nonprog_dsk_loads_if_available() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("disk")
            .join("nonprog.dsk");
        if !path.exists() {
            eprintln!(
                "[real_nonprog_dsk_loads_if_available] skipping — disk image not present at {path:?}"
            );
            return;
        }
        let img = load_disk_image_from_file(&path).expect("load real .dsk");
        assert_eq!(img.sectors.len(), TOTAL_SECTORS);

        // Sector (0, 0, 0) is the boot sector.  The boot loader
        // microcode reads its 256-word data payload into Nova memory
        // locations 1..257 (memory location 0 is the reset vector and
        // is never overwritten by the boot block).  So `data[0]` of
        // the disk sector corresponds to memory address 001 — the
        // JMP-345 entrypoint per the boot block disassembly:
        //
        //   001: 000345 JMP 345 ; Entrypoint
        //   002: 000354 JMP 354
        //   003: 000403 JMP .+3
        //
        // Octal 000345 = 0xE5.
        let boot = img.sector(0, 0, 0);
        assert_ne!(
            boot.data, [0u16; DATA_WORDS],
            "boot sector data must not be all-zero"
        );
        assert_eq!(
            boot.data[0], 0o000345,
            "boot sector data[0] must be the JMP-345 entrypoint (memory addr 001)"
        );
        assert_eq!(
            boot.data[1], 0o000354,
            "boot sector data[1] must match boot block address 002"
        );
        assert_eq!(
            boot.data[2], 0o000403,
            "boot sector data[2] must match boot block address 003"
        );
        // Header is currently zero (no checksum/disk-address recorded);
        // ContrAlto fills these in at runtime from KADR.  Just confirm
        // the structure parsed without complaint.
        assert_eq!(boot.header.len(), HEADER_WORDS);
        assert_eq!(boot.label.len(), LABEL_WORDS);
        assert_eq!(boot.data.len(), DATA_WORDS);
    }
}
