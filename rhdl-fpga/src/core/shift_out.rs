//! Shift-Out Register (Parallel to Serial Converter)
//!
//! A shift-out register that loads parallel data when the load signal is high,
//! and shifts data out serially (MSB first) on each clock cycle when enabled.
//! The [ShiftOut] is parameterized by the bit width `N` of the internal storage register.
//!
//! The [ShiftOut] is commonly used for parallel-to-serial conversion,
//! taking parallel data words and transmitting them as serial bit streams.
//!
//! Here is the schematic symbol
//!
#![doc = badascii_doc::badascii_formal!(r"
     +-+ShiftOut<N>+----+      
     |                  |      
bool |          serial  |bool  
+--->+ enable     out   +----->
     |                  |      
bool |                  |      
+--->+ load             |      
     |                  |      
B<N> |                  |      
+--->+ data_in          |      
     |                  |      
     +------------------+      
")]
//!
//! # Operation
//!
//! On each positive clock edge:
//! 1. If `load` is high: The parallel `data_in` is loaded into the register
//! 2. Else if `enable` is high: The register shifts left by one position, outputting the MSB
//! 3. Else: The register holds its current value
//! 4. The `serial_out` always reflects the current MSB of the register
//!
//! The shifting behavior can be visualized as:
//!
#![doc = badascii_doc::badascii!(r"
    Before:  [MSB] [6] [5] [4] [3] [2] [1] [LSB]
               |
               v (serial_out)
    After:   [6] [5] [4] [3] [2] [1] [LSB] [0]
")]
//!
//! # Example
//!
//! Here's a simple example of a shift-out register.
//!```
#![doc = include_str!("../../examples/shift_out.rs")]
//!```
use rhdl::prelude::*;

use super::dff;

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
/// A parallel-to-serial shift-out register
///   `N` is the bit width of the shift register
pub struct ShiftOut<N: BitWidth> {
    register: dff::DFF<Bits<N>>,
}

impl<N: BitWidth> Default for ShiftOut<N> {
    fn default() -> Self {
        Self {
            register: dff::DFF::new(Bits::<N>::default()),
        }
    }
}

/// Inputs for the ShiftOut register: (enable, load, data_in)
pub type OutIn<N> = (bool, bool, Bits<N>);

impl<N: BitWidth> SynchronousIO for ShiftOut<N> {
    type I = OutIn<N>; // (enable, load, data_in)
    type O = bool; // Serial output (MSB of current register)
    type Kernel = shift_out_kernel<N>;
}

#[kernel]
/// Shift-out register kernel function
pub fn shift_out_kernel<N: BitWidth>(cr: ClockReset, input: OutIn<N>, q: Q<N>) -> (bool, D<N>) {
    let (enable, load, data_in) = input;

    let current_reg = q.register;

    // Output the MSB (most significant bit)
    let serial_out = (current_reg.val >> (N::BITS - 1)) != 0;

    let next_reg = if cr.reset.any() {
        bits(0) // Reset to all zeros
    } else if load {
        data_in // Load new parallel data
    } else if enable {
        // Shift left by 1, filling LSB with 0
        current_reg << 1
    } else {
        current_reg // Hold current value when disabled
    };

    (serial_out, D::<N> { register: next_reg })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shift_out_load_and_shift() -> miette::Result<()> {
        // Create an 8-bit shift-out register
        let uut: ShiftOut<U8> = ShiftOut::default();

        // Test sequence: load 0xAB, then shift out bit by bit
        let test_data = vec![
            (false, true, bits(0xAB)),  // Load 0xAB (0b10101011)
            (true, false, bits(0x00)),  // Shift: output MSB=1, reg becomes 0x56 (0b01010110)
            (true, false, bits(0x00)),  // Shift: output MSB=0, reg becomes 0xAC (0b10101100)
            (true, false, bits(0x00)),  // Shift: output MSB=1, reg becomes 0x58 (0b01011000)
            (true, false, bits(0x00)),  // Shift: output MSB=0, reg becomes 0xB0 (0b10110000)
            (true, false, bits(0x00)),  // Shift: output MSB=1, reg becomes 0x60 (0b01100000)
            (true, false, bits(0x00)),  // Shift: output MSB=0, reg becomes 0xC0 (0b11000000)
            (true, false, bits(0x00)),  // Shift: output MSB=1, reg becomes 0x80 (0b10000000)
            (true, false, bits(0x00)),  // Shift: output MSB=1, reg becomes 0x00 (0b00000000)
            (false, false, bits(0x00)), // Disabled - should hold at 0x00
        ];

        let input = test_data.with_reset(2).clock_pos_edge(100);
        let output_stream = uut.run(input)?;
        let outputs: Vec<_> = output_stream.map(|t| t.value.2).collect();

        // Check the bit sequence output (MSB first from 0xAB = 0b10101011)
        assert_eq!(outputs[2], true);  // 1st bit: 1 (MSB of 0xAB)
        assert_eq!(outputs[3], false); // 2nd bit: 0
        assert_eq!(outputs[4], true);  // 3rd bit: 1
        assert_eq!(outputs[5], false); // 4th bit: 0
        assert_eq!(outputs[6], true);  // 5th bit: 1
        assert_eq!(outputs[7], false); // 6th bit: 0
        assert_eq!(outputs[8], true);  // 7th bit: 1
        assert_eq!(outputs[9], true);  // 8th bit: 1 (LSB of 0xAB)

        // When disabled, should hold the same output
        assert_eq!(outputs[10], false); // Register is now 0x00, so MSB is 0

        Ok(())
    }

    #[test]
    fn test_shift_out_reload() -> miette::Result<()> {
        // Test loading new data mid-sequence
        let uut: ShiftOut<U4> = ShiftOut::default();

        let test_data = vec![
            (false, true, bits(0xF)),   // Load 0xF (0b1111)
            (true, false, bits(0x0)),   // Shift: output 1, reg becomes 0xE (0b1110)
            (true, false, bits(0x0)),   // Shift: output 1, reg becomes 0xC (0b1100)
            (false, true, bits(0x5)),   // Load 0x5 (0b0101) - interrupts shifting
            (true, false, bits(0x0)),   // Shift: output 0, reg becomes 0xA (0b1010)
            (true, false, bits(0x0)),   // Shift: output 1, reg becomes 0x4 (0b0100)
        ];

        let input = test_data.with_reset(1).clock_pos_edge(100);
        let output_stream = uut.run(input)?;
        let outputs: Vec<_> = output_stream.map(|t| t.value.2).collect();

        // Check the sequence
        assert_eq!(outputs[1], true);  // MSB of 0xF = 1
        assert_eq!(outputs[2], true);  // MSB of 0xE = 1
        assert_eq!(outputs[3], true);  // MSB of 0xC = 1
        assert_eq!(outputs[4], false); // MSB of 0x5 = 0 (newly loaded)
        assert_eq!(outputs[5], true);  // MSB of 0xA = 1
        assert_eq!(outputs[6], false); // MSB of 0x4 = 0

        Ok(())
    }

    #[test]
    fn test_shift_out_priority() -> miette::Result<()> {
        // Test that load has priority over enable
        let uut: ShiftOut<U4> = ShiftOut::default();

        let test_data = vec![
            (false, true, bits(0xA)),   // Load 0xA (0b1010)
            (true, true, bits(0x5)),    // Both enable and load high - load should win
        ];

        let input = test_data.with_reset(1).clock_pos_edge(100);
        let output_stream = uut.run(input)?;
        let outputs: Vec<_> = output_stream.map(|t| t.value.2).collect();

        assert_eq!(outputs[1], true);  // MSB of 0xA = 1
        assert_eq!(outputs[2], false); // MSB of 0x5 = 0 (load took priority over shift)

        Ok(())
    }
}