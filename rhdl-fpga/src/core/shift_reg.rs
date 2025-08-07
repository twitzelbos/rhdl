//! Shift-In Register (Serial to Parallel Converter)
//!
//! A shift-in register that shifts serial data left on each clock cycle
//! when enabled, and outputs the current parallel value. New bits are shifted
//! in from the LSB position. The [ShiftRegister] is parameterized by the
//! bit width `N` of the internal storage register.
//!
//! The [ShiftRegister] is commonly used for serial-to-parallel conversion,
//! receiving serial data streams and converting them to parallel words.
//!
//! Here is the schematic symbol
//!
#![doc = badascii_doc::badascii_formal!(r"
      +-+ShiftIn<N>+----+      
      |                 |      
 bool |         data    |B<N>  
+---->+ enable    out   +----->
      |                 |      
 bool |                 |      
+---->+ serial_in       |      
      |                 |      
      +-----------------+      
")]
//!
//! # Operation
//!
//! On each positive clock edge (when enabled), the shift register performs:
//! 1. The MSB is output (before shifting)
//! 2. All bits shift left by one position  
//! 3. The `serial_in` bit fills the LSB position
//! 4. When disabled, the register holds its current value
//!
//! The shifting behavior can be visualized as:
//!
#![doc = badascii_doc::badascii!(r"
     Before:  [MSB] [6] [5] [4] [3] [2] [1] [LSB]
                |
                v (output)
     After:   [6] [5] [4] [3] [2] [1] [LSB] [serial_in]
")]
//!
//! # Example
//!
//! Here's a simple example of a shift register.
//!```
#![doc = include_str!("../../examples/shift_register.rs")]
//!```
use rhdl::prelude::*;

use super::dff;

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
/// A bit-shifting shift register
///   `N` is the bit width of the shift register
pub struct ShiftRegister<N: BitWidth> {
    register: dff::DFF<Bits<N>>,
}

impl<N: BitWidth> Default for ShiftRegister<N> {
    fn default() -> Self {
        Self {
            register: dff::DFF::new(Bits::<N>::default()),
        }
    }
}

/// Inputs for the FIFO
pub type In = (bool, bool); // (enable, serial_in)

impl<N: BitWidth> SynchronousIO for ShiftRegister<N> {
    type I = In; // (enable, serial_in)
    type O = Bits<N>; // Parallel output of current register value
    type Kernel = shift_register_kernel<N>;
}

#[kernel]
/// Shift-in register kernel function
pub fn shift_register_kernel<N: BitWidth>(cr: ClockReset, input: In, q: Q<N>) -> (Bits<N>, D<N>) {
    let (enable, serial_in) = input;

    let current_reg = q.register;

    // Output the current parallel value
    let parallel_out = current_reg;

    let next_reg = if cr.reset.any() {
        bits(0) // Reset to all zeros
    } else if enable {
        // Shift left by 1 and OR in the new LSB
        let shifted_left = current_reg << 1;
        let serial_bit = if serial_in { bits(1) } else { bits(0) };
        shifted_left | serial_bit
    } else {
        current_reg // Hold current value when disabled
    };

    (parallel_out, D::<N> { register: next_reg })
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;

    #[test]
    fn test_shift_register_basic() -> miette::Result<()> {
        // Create an 8-bit shift register
        let uut: ShiftRegister<U8> = ShiftRegister::default();

        // Shift in the pattern 1,0,1,0,1,1,0,0 (should build up to 0xAC)
        let test_data = vec![
            (true, true),  // Shift in 1 -> register = 0x01, output = 0x01
            (true, false), // Shift in 0 -> register = 0x02, output = 0x02
            (true, true),  // Shift in 1 -> register = 0x05, output = 0x05
            (true, false), // Shift in 0 -> register = 0x0A, output = 0x0A
            (true, true),  // Shift in 1 -> register = 0x15, output = 0x15
            (true, true),  // Shift in 1 -> register = 0x2B, output = 0x2B
            (true, false), // Shift in 0 -> register = 0x56, output = 0x56
            (true, false), // Shift in 0 -> register = 0xAC, output = 0xAC
            (false, true), // Disabled - should hold at 0xAC
        ];

        let input = test_data.with_reset(2).clock_pos_edge(100);
        let output_stream = uut.run(input)?;
        let outputs: Vec<_> = output_stream.map(|t| t.value.2).collect();

        // Check the progressive build-up of the pattern
        assert_eq!(outputs[2], bits(0x01)); // After shifting in first 1
        assert_eq!(outputs[3], bits(0x02)); // After shifting in 0
        assert_eq!(outputs[4], bits(0x05)); // After shifting in 1
        assert_eq!(outputs[5], bits(0x0A)); // After shifting in 0
        assert_eq!(outputs[6], bits(0x15)); // After shifting in 1
        assert_eq!(outputs[7], bits(0x2B)); // After shifting in 1
        assert_eq!(outputs[8], bits(0x56)); // After shifting in 0
        assert_eq!(outputs[9], bits(0xAC)); // After shifting in 0 - complete pattern!

        // When disabled, should hold the same value
        assert_eq!(outputs[10], bits(0xAC)); // Should hold value when disabled

        Ok(())
    }

    #[test]
    fn test_shift_register_pattern() -> miette::Result<()> {
        // Test with a 4-bit shift register for easier verification
        let uut: ShiftRegister<U4> = ShiftRegister::default();

        // Shift in pattern 1,0,1,1 -> builds up: 0x1, 0x2, 0x5, 0xB
        let test_data = vec![
            (true, true),  // register = 0x1, output = 0x1
            (true, false), // register = 0x2, output = 0x2
            (true, true),  // register = 0x5, output = 0x5
            (true, true),  // register = 0xB, output = 0xB
            (true, false), // register = 0x6, output = 0x6 (0xB << 1, LSB = 0, wraps at 4-bit)
        ];

        let input = test_data.with_reset(1).clock_pos_edge(100);
        let output_stream = uut.run(input)?;
        let outputs: Vec<_> = output_stream.map(|t| t.value.2).collect();

        // Check the progressive pattern build-up
        assert_eq!(outputs[1], bits(0x1)); // After shifting in 1
        assert_eq!(outputs[2], bits(0x2)); // After shifting in 0
        assert_eq!(outputs[3], bits(0x5)); // After shifting in 1
        assert_eq!(outputs[4], bits(0xB)); // After shifting in 1 -> 1011
        assert_eq!(outputs[5], bits(0x6)); // After shifting in 0 -> 0110 (MSB shifted out)

        Ok(())
    }

    #[test]
    fn test_shift_register_reset() -> miette::Result<()> {
        let uut: ShiftRegister<U4> = ShiftRegister::default();

        // Load some data, then reset
        let test_data = vec![
            (true, true), // Load some 1s
            (true, true),
            (true, true),
            (true, true),  // Register should be all 1s now (0xF)
            (true, false), // Should shift in 0
        ];

        let input = test_data.with_reset(3).clock_pos_edge(100); // Reset for 3 cycles
        let output_stream = uut.run(input)?;
        let outputs: Vec<_> = output_stream.map(|t| t.value.2).collect();

        // After reset, register should start at 0
        assert_eq!(outputs[3], bits(0)); // First output after reset should be 0

        // Then build up as we shift in 1s
        assert_eq!(outputs[4], bits(0x1)); // After first 1
        assert_eq!(outputs[5], bits(0x3)); // After second 1
        assert_eq!(outputs[6], bits(0x7)); // After third 1
        assert_eq!(outputs[7], bits(0xF)); // After fourth 1 -> all 1s
        assert_eq!(outputs[8], bits(0xE)); // After shifting in 0 -> 1110

        Ok(())
    }
}
