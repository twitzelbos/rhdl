# Converting Integer Bits<N> to IEEE 754 Float32

Converting an integer bit vector to IEEE 754 single-precision float format requires implementing the float encoding algorithm in hardware. Here's how you could do it in RHDL.

## IEEE 754 Float32 Format

```
31    30-23      22-0
S     Exponent   Mantissa
|     (8 bits)   (23 bits)
Sign bit
```

- **Sign**: 1 bit (0 = positive, 1 = negative)
- **Exponent**: 8 bits (biased by 127)
- **Mantissa**: 23 bits (fractional part, with implicit leading 1)

## RHDL Implementation

```rust
use rhdl::prelude::*;

#[kernel]
/// Convert unsigned integer to IEEE 754 float32
/// Returns 32-bit representation of the float
pub fn int_to_float32<N: BitWidth>(
    _cr: ClockReset,
    input: Bits<N>,
    _q: ()
) -> (Bits<U32>, ()) {
    
    // Handle special case of zero
    if input == bits(0) {
        return (bits(0u32), ()); // +0.0
    }
    
    // Find the position of the most significant bit (leading one)
    let mut msb_pos = 0u32;
    let mut temp = input;
    
    // Count leading zeros to find MSB position
    // This is a hardware-synthesizable way to find bit position
    for i in 0..(N::BITS as u32) {
        if (temp >> (N::BITS - 1 - i as usize)) & bits(1) == bits(1) {
            msb_pos = (N::BITS as u32) - 1 - i;
            break;
        }
    }
    
    // IEEE 754 components
    let sign = bits(0u32); // Always positive for unsigned input
    
    // Exponent: bias (127) + bit position  
    let exponent = if msb_pos <= 127 {
        bits((127 + msb_pos) << 23)
    } else {
        bits(0xFFu32 << 23) // Overflow to infinity
    };
    
    // Mantissa: extract fractional part (23 bits after the implicit leading 1)
    let mantissa = if msb_pos >= 23 {
        // Shift right to get 23 bits after MSB
        let shift_amount = msb_pos - 23;
        (input >> shift_amount) & bits((1u32 << 23) - 1)
    } else {
        // Shift left to fill 23 bits  
        let shift_amount = 23 - msb_pos;
        (input << shift_amount) & bits((1u32 << 23) - 1)
    };
    
    // Combine components
    let result = sign | exponent | mantissa;
    
    (result, ())
}

#[kernel] 
/// Convert signed integer to IEEE 754 float32
pub fn signed_int_to_float32<N: BitWidth>(
    _cr: ClockReset,
    input: SignedBits<N>,
    _q: ()
) -> (Bits<U32>, ()) {
    
    // Handle sign
    let is_negative = input < signed_bits(0);
    let abs_input = if is_negative { 
        -input 
    } else { 
        input 
    };
    
    // Convert absolute value using unsigned conversion
    let (mut float_bits, _) = int_to_float32(_cr, abs_input.as_unsigned(), ());
    
    // Set sign bit if negative
    if is_negative {
        float_bits = float_bits | bits(0x8000_0000u32);
    }
    
    (float_bits, ())
}

// Helper function for testing
pub fn bits_to_f32(bits: Bits<U32>) -> f32 {
    f32::from_bits(bits.val as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_int_to_float32() {
        // Test zero
        let (result, _) = int_to_float32(ClockReset::default(), bits(0u32), ());
        assert_eq!(bits_to_f32(result), 0.0f32);
        
        // Test small integers
        let (result, _) = int_to_float32(ClockReset::default(), bits(1u32), ());
        assert_eq!(bits_to_f32(result), 1.0f32);
        
        let (result, _) = int_to_float32(ClockReset::default(), bits(42u32), ());
        assert_eq!(bits_to_f32(result), 42.0f32);
        
        // Test power of 2
        let (result, _) = int_to_float32(ClockReset::default(), bits(256u32), ());
        assert_eq!(bits_to_f32(result), 256.0f32);
    }
    
    #[test]
    fn test_signed_int_to_float32() {
        // Test positive
        let (result, _) = signed_int_to_float32(
            ClockReset::default(), 
            signed_bits(42i32), 
            ()
        );
        assert_eq!(bits_to_f32(result), 42.0f32);
        
        // Test negative  
        let (result, _) = signed_int_to_float32(
            ClockReset::default(),
            signed_bits(-42i32), 
            ()
        );
        assert_eq!(bits_to_f32(result), -42.0f32);
    }
}
```

## Usage Example

```rust
use rhdl::prelude::*;

// Create a simple converter circuit
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
pub struct IntToFloatConverter<N: BitWidth> {
    // No internal state needed for pure conversion
}

impl<N: BitWidth> Default for IntToFloatConverter<N> {
    fn default() -> Self {
        Self {}
    }
}

impl<N: BitWidth> SynchronousIO for IntToFloatConverter<N> {
    type I = Bits<N>;        // Input: N-bit integer
    type O = Bits<U32>;      // Output: 32-bit IEEE 754 float
    type Kernel = int_to_float32<N>;
}

// Example usage
fn main() -> Result<(), RHDLError> {
    let converter: IntToFloatConverter<U16> = IntToFloatConverter::default();
    
    let test_values = vec![
        bits(0u16),
        bits(1u16), 
        bits(42u16),
        bits(255u16),
        bits(1024u16),
    ];
    
    let input = test_values
        .with_reset(2)
        .clock_pos_edge(100);
        
    let output_stream = converter.run(input)?;
    
    for sample in output_stream {
        let input_val = sample.value.1;
        let float_bits = sample.value.2;
        let float_val = f32::from_bits(float_bits.val as u32);
        
        println!("Input: {} -> Float bits: 0x{:08X} -> Float: {}", 
                 input_val.val, float_bits.val, float_val);
    }
    
    Ok(())
}
```

## Key Considerations

### Hardware Constraints
- **No floating-point operations**: All operations use integer arithmetic
- **Bit manipulation**: Uses shifts, masks, and boolean logic only  
- **Synthesizable**: Can be converted to hardware (Verilog/VHDL)
- **Deterministic**: No runtime floating-point library dependencies

### Precision Limitations
- **Mantissa precision**: Only 23 bits of precision in float32
- **Large integers**: May lose precision if input has > 24 significant bits
- **Rounding**: This implementation truncates; IEEE 754 specifies rounding

### Performance
- **Combinational logic**: Conversion happens in single clock cycle
- **Resource usage**: Requires shifters and logic for bit manipulation
- **Pipelined version**: Could be split across multiple clock cycles for better timing

## Alternative: Pipelined Version

For better timing in hardware, you could create a pipelined version:

```rust
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]  
pub struct PipelinedIntToFloat<N: BitWidth> {
    stage1: dff::DFF<Bits<N>>,           // Input register
    stage2: dff::DFF<(Bits<U8>, Bits<U5>)>, // Exponent and MSB position  
    stage3: dff::DFF<Bits<U32>>,         // Final result
}

// Implementation would spread the conversion across 3 clock cycles
// for better timing closure in hardware
```

## Summary

This approach:
- ✅ **Pure RHDL**: Uses only synthesizable operations
- ✅ **Type safe**: Leverages Bits<N> compile-time width checking
- ✅ **Hardware ready**: Can be converted to Verilog/VHDL
- ✅ **Testable**: Can be simulated and verified before synthesis
- ⚠️ **Precision**: Limited by IEEE 754 mantissa precision
- ⚠️ **Complexity**: Requires careful bit manipulation logic

The key insight is that **all floating-point format manipulation becomes integer bit manipulation** in hardware, which RHDL handles excellently with its `Bits<N>` types and kernel functions!