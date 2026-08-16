//! Alto Constant ROM — combinational 256 × 16-bit lookup.
//!
//! When the microinstruction's F1 = Constant code is asserted, the
//! BUS is driven from the Constant ROM at index `RSEL[4:0] ++ BS[2:0]`
//! (8-bit address → 256 entries).  The lookup is **combinational** —
//! same cycle as the rest of the microinstruction execution — so this
//! widget is a pure-combinational kernel over a stored constant table.
//!
//! ## Phase-3.5 capabilities
//!
//! - **256 entries × 16 bits**, all readable in the same cycle.
//! - **Initial contents** loaded from the [`crate::microcode_loader`]
//!   `load_alto_ii_constant_rom` output via [`ConstantRom::with_constants`].
//! - **Pure combinational** — the kernel has no state.
//!
//! ## Composition
//!
//! ```ignore
//! use rhdl_alto::{constant_rom::ConstantRom, microcode_loader};
//! let constants = microcode_loader::load_alto_ii_constant_rom_from_dir(&dir)?;
//! let rom = ConstantRom::with_constants(&constants);
//! ```

use rhdl::prelude::*;

/// Total entries in the Constant ROM.
pub const NUM_CONSTANTS: usize = 256;

/// Inputs to the Constant ROM.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct ConstantIn {
    /// 8-bit lookup index — composed by the microengine from
    /// `(RSEL[4:0] << 3) | BS[2:0]`.
    pub index: Bits<8>,
}

/// Outputs from the Constant ROM.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct ConstantOut {
    /// Constant at the supplied index this cycle.
    pub value: Bits<16>,
}

/// 256 × 16-bit Constant ROM.
///
/// Stores all 256 constants as fields of a [`Digital`]-derived struct
/// inside a single DFF-equivalent (initialised at construction; never
/// written).  Combinational lookup at the kernel level.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct ConstantRom {
    /// The 256-entry constant table.  Stored as a `Constant` so the
    /// emitted Verilog has the values as compile-time constants
    /// (no register file).
    table: rhdl_fpga::core::constant::Constant<[Bits<16>; 256]>,
}

impl Default for ConstantRom {
    fn default() -> Self {
        Self {
            table: rhdl_fpga::core::constant::Constant::new([bits::<16>(0); 256]),
        }
    }
}

impl ConstantRom {
    /// Construct a Constant ROM with the supplied 256-entry table.
    /// Pass the output of [`crate::microcode_loader::load_alto_ii_constant_rom`]
    /// directly.
    pub fn with_constants(constants: &[u16; NUM_CONSTANTS]) -> Self {
        let mut table = [bits::<16>(0); 256];
        for (i, &c) in constants.iter().enumerate() {
            table[i] = bits::<16>(c as u128);
        }
        Self {
            table: rhdl_fpga::core::constant::Constant::new(table),
        }
    }
}

impl SynchronousIO for ConstantRom {
    type I = ConstantIn;
    type O = ConstantOut;
    type Kernel = constant_rom_kernel;
}

#[kernel]
pub fn constant_rom_kernel(_cr: ClockReset, i: ConstantIn, q: Q) -> (ConstantOut, D) {
    let d = D::dont_care();
    let mut o = ConstantOut::dont_care();

    o.value = q.table[i.index];

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b8(v: u8) -> Bits<8> {
        bits::<8>(v as u128)
    }
    fn b16(v: u16) -> Bits<16> {
        bits::<16>(v as u128)
    }

    fn run_inputs(uut: ConstantRom, inputs: Vec<ConstantIn>) -> Vec<ConstantOut> {
        let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
        uut.run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect()
    }

    #[test]
    fn combinational_lookup() {
        // Preload three known entries.
        let mut constants = [0u16; NUM_CONSTANTS];
        constants[0] = 0x1111;
        constants[42] = 0xCAFE;
        constants[255] = 0xBEEF;
        let uut = ConstantRom::with_constants(&constants);
        let trace = run_inputs(
            uut,
            vec![
                ConstantIn { index: b8(0) },
                ConstantIn { index: b8(42) },
                ConstantIn { index: b8(255) },
                ConstantIn { index: b8(7) }, // unset → 0
            ],
        );
        // Combinational: output == this cycle's input.
        assert_eq!(trace[0].value, b16(0x1111));
        assert_eq!(trace[1].value, b16(0xCAFE));
        assert_eq!(trace[2].value, b16(0xBEEF));
        assert_eq!(trace[3].value, b16(0));
    }

    /// Real-PROM integration: load actual Alto II Constant ROM and
    /// confirm the loader output is consumable.
    #[test]
    fn load_real_constant_rom_and_lookup() {
        use crate::microcode_loader;
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("rom");
        if !dir.join("C0").exists() {
            eprintln!("[load_real_constant_rom_and_lookup] skipping — Constant ROMs absent");
            return;
        }
        let constants = microcode_loader::load_alto_ii_constant_rom_from_dir(&dir)
            .expect("load real Constant ROMs");
        let uut = ConstantRom::with_constants(&constants);
        // Look up the first 4 constants and verify they match the loader output.
        let trace = run_inputs(
            uut,
            vec![
                ConstantIn { index: b8(0) },
                ConstantIn { index: b8(1) },
                ConstantIn { index: b8(2) },
                ConstantIn { index: b8(3) },
            ],
        );
        for i in 0..4 {
            assert_eq!(
                trace[i].value,
                b16(constants[i]),
                "Constant ROM[{i}] mismatch: trace[{i}] = {:?}, expected {:#06x}",
                trace[i].value,
                constants[i]
            );
        }
    }

    #[test]
    fn constant_rom_iverilog_round_trip() -> Result<(), RHDLError> {
        let mut constants = [0u16; NUM_CONSTANTS];
        for i in 0..16 {
            constants[i] = (0xA000 + i) as u16;
        }
        let uut = ConstantRom::with_constants(&constants);
        let inputs: Vec<ConstantIn> = (0..6).map(|i| ConstantIn { index: b8(i as u8) }).collect();
        let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }
}
