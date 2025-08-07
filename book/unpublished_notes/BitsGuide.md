# Working with Bits<N> in RHDL

This guide covers the essential operations for working with RHDL's `Bits<N>` type - the foundation for all hardware bit manipulation.

## Creating Bits<N>

### From Literals
```rust
use rhdl::prelude::*;

// Using the bits() function (most common)
let val8: Bits<U8> = bits(0x42u8);     // 8-bit value 0x42
let val16: Bits<U16> = bits(0xABCDu16); // 16-bit value 0xABCD
let val32: Bits<U32> = bits(123456u32); // 32-bit value

// Using From trait
let val8: Bits<U8> = Bits::from(0x42u8);
let val16: Bits<U16> = 0xABCDu16.into();

// Binary literals
let val4: Bits<U4> = bits(0b1010u8);    // 4-bit binary
let val8: Bits<U8> = bits(0b11110000u8); // 8-bit binary

// Hex literals  
let val16: Bits<U16> = bits(0xDEADu16);
let val32: Bits<U32> = bits(0xDEAD_BEEFu32);
```

### Default Values
```rust
// Zero initialization
let zero8: Bits<U8> = Bits::default();     // All zeros
let zero16: Bits<U16> = Bits::<U16>::ZERO; // Explicit zero constant

// Maximum values
let max8: Bits<U8> = Bits::<U8>::mask();   // All ones (0xFF)
let max16: Bits<U16> = Bits::<U16>::mask(); // All ones (0xFFFF)
```

## Concatenating Bits<N> and Bits<M> → Bits<N+M>

**Important**: RHDL currently does **not** provide built-in concatenation functions. You must implement concatenation manually using shifts and bitwise operations.

### Manual Concatenation Using Shifts (The Current Solution)

#### Basic Pattern
```rust
use rhdl::prelude::*;

let high: Bits<U8> = bits(0xABu8);   // High 8 bits
let low: Bits<U8> = bits(0xCDu8);    // Low 8 bits

// Manual concatenation: shift high bits left by low's width, then OR
let combined: Bits<U16> = bits((high.val << 8) | low.val); // Result: 0xABCD
//                              ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
//                              This is what cat() would do internally
```

#### Different Bit Widths
```rust
let high: Bits<U8> = bits(0xABu8);   // 8 bits
let low: Bits<U4> = bits(0xCu8);     // 4 bits

// Result is 8 + 4 = 12 bits
let combined: Bits<U12> = bits((high.val << 4) | low.val); // Result: 0xABC
```

#### The General Formula
```rust
// For concatenating Bits<N> and Bits<M>:
let result: Bits<{N+M}> = bits((high.val << M) | low.val);
//                               ^^^^^^^^^^^^ ^^^^^^^^^^^
//                               Shift high   OR with low
//                               left by M    (unchanged)
```

### Practical Helper Functions (Workaround)

Since generic concatenation is impossible, create specific functions for common cases:

```rust
use rhdl::prelude::*;

// Helper functions for common bit width combinations
fn concat_8_8(high: Bits<U8>, low: Bits<U8>) -> Bits<U16> {
    bits((high.val << 8) | low.val)
}

fn concat_4_4(high: Bits<U4>, low: Bits<U4>) -> Bits<U8> {
    bits((high.val << 4) | low.val)
}

fn concat_8_4(high: Bits<U8>, low: Bits<U4>) -> Bits<U12> {
    bits((high.val << 4) | low.val)
}

// Usage examples
let high8 = bits(0xABu8);
let low8 = bits(0xCDu8);
let result16 = concat_8_8(high8, low8); // 0xABCD

let high4 = bits(0xAu8);
let low4 = bits(0xBu8);
let result8 = concat_4_4(high4, low4);   // 0xAB
```

#### Kernel Function for Hardware Synthesis
```rust
#[kernel]
fn concat_8_8_kernel(
    _cr: ClockReset,
    input: (Bits<U8>, Bits<U8>),
    _q: ()
) -> (Bits<U16>, ()) {
    let (high, low) = input;
    let result = bits((high.val << 8) | low.val);
    (result, ())
}
```

### Multiple Concatenations
```rust
// Build up larger words by chaining manual concatenations
let a: Bits<U4> = bits(0xAu8);
let b: Bits<U4> = bits(0xBu8);  
let c: Bits<U4> = bits(0xCu8);
let d: Bits<U4> = bits(0xDu8);

// Method 1: Step by step
let ab: Bits<U8> = bits((a.val << 4) | b.val);      // 0xAB (4+4=8 bits)
let cd: Bits<U8> = bits((c.val << 4) | d.val);      // 0xCD (4+4=8 bits)
let abcd: Bits<U16> = bits((ab.val << 8) | cd.val); // 0xABCD (8+8=16 bits)

// Method 2: Direct (more efficient)
let abcd: Bits<U16> = bits((a.val << 12) | (b.val << 8) | (c.val << 4) | d.val);
//                          ^^^^^^^^^^^   ^^^^^^^^^^^   ^^^^^^^^^^^   ^^^^^^^
//                          A at bits    B at bits     C at bits     D at bits
//                          [15:12]      [11:8]        [7:4]         [3:0]
```

#### Real-World Example: Building a 32-bit Instruction
```rust
let opcode: Bits<U6> = bits(0x20u8);    // 6 bits: ADD instruction
let rs1: Bits<U5> = bits(0x10u8);       // 5 bits: source register 1
let rs2: Bits<U5> = bits(0x11u8);       // 5 bits: source register 2  
let rd: Bits<U5> = bits(0x01u8);        // 5 bits: destination register
let funct: Bits<U11> = bits(0x000u16);  // 11 bits: function code

// Build 32-bit RISC-V instruction format
let instruction: Bits<U32> = bits(
    (opcode.val << 26) |  // Bits [31:26]
    (rs1.val << 21) |     // Bits [25:21]  
    (rs2.val << 16) |     // Bits [20:16]
    (rd.val << 11) |      // Bits [15:11]
    funct.val             // Bits [10:0]
);

println!("Instruction: 0x{:08X}", instruction.val);
```

### Concatenating Single Bits
```rust
let bit1: Bits<U1> = bits(1u8);
let bit0: Bits<U1> = bits(0u8);
let byte: Bits<U8> = bits(0x42u8);

// Build larger words bit by bit using shifts
let two_bits: Bits<U2> = bits((bit1.val << 1) | bit0.val); // 0b10
let result: Bits<U10> = bits((two_bits.val << 8) | byte.val); // 10-bit result
```

#### Utility Macro (Optional)
```rust
macro_rules! concat_bits {
    ($high:expr, $low:expr, $low_width:literal) => {
        bits(($high.val << $low_width) | $low.val)
    };
}

// Usage
let high: Bits<U8> = bits(0xABu8);
let low: Bits<U4> = bits(0xCu8);
let result: Bits<U12> = concat_bits!(high, low, 4); // 0xABC
```

### Why No Built-in Concatenation?

The fundamental reason RHDL cannot provide a `cat()` function is a **Rust language limitation**, not a design choice.

#### The Ideal Function (Impossible in Stable Rust)
```rust
// This would be the ideal concatenation function:
fn cat<const N: usize, const M: usize>(high: Bits<N>, low: Bits<M>) -> Bits<{N + M}>
//                                                                          ^^^^^^^
//                                                                          This arithmetic is forbidden in stable Rust
```

#### Rust Const Generic Limitations

**Stable Rust (Current):**
```rust
fn cat<const N: usize, const M: usize>(high: Bits<N>, low: Bits<M>) -> Bits<{N + M}> {
//                                                                          ^^^^^^^
// Error: generic parameters may not be used in const operations
```

**Nightly Rust (Unstable):**
```rust
#![feature(generic_const_exprs)]  // ⚠️ Incomplete, may cause compiler crashes

fn cat<const N: usize, const M: usize>(high: Bits<N>, low: Bits<M>) -> Bits<{N + M}>
where
    [(); N + M]:,  // Assert that N + M is a valid const
{
    Bits((high.val << M) | low.val)  // ✅ This actually works on nightly
}
```

#### Why RHDL Can't Use Nightly Features

1. **Production Stability**: Libraries cannot depend on unstable compiler features
2. **Compiler Crashes**: `generic_const_exprs` is explicitly marked as crash-prone
3. **User Compatibility**: Most users are on stable Rust
4. **Long-term Maintenance**: Nightly features can break or change without warning

#### Additional Technical Challenges

Even with stable const generics, RHDL faces:

1. **Hardware Synthesis Complexity**: Each `cat::<N, M>()` needs specialized Verilog generation
2. **Type Inference Issues**: Without explicit annotations, result types become ambiguous
3. **Trait System Limitations**: Implementing concatenation as a trait is extremely complex

#### When Will This Be Fixed?

The `generic_const_exprs` feature has been in development for **years** with no clear stabilization timeline. See [Rust Issue #76560](https://github.com/rust-lang/rust/issues/76560).

Until then, **manual concatenation is the only reliable approach** in stable Rust.

## Initializing from Literals

### Type Inference
```rust
// Let Rust infer the literal type
let val = bits(42);        // Defaults to some integer type
let val: Bits<U8> = bits(42); // Explicit type annotation

// For specific bit widths, be explicit about literal type
let val4: Bits<U4> = bits(15u8);   // 4-bit max value
let val12: Bits<U12> = bits(4095u16); // 12-bit max value
```

### Literal Constraints
```rust
// ✅ This works - value fits in bit width
let val4: Bits<U4> = bits(15u8);  // 15 fits in 4 bits

// ❌ This panics - value too large for bit width  
// let bad: Bits<U4> = bits(16u8); // 16 doesn't fit in 4 bits
```

### Common Patterns
```rust
// Powers of 2
let pow8: Bits<U16> = bits(256u16);    // 2^8
let pow10: Bits<U16> = bits(1024u16);  // 2^10

// Bit patterns
let alternating: Bits<U8> = bits(0b10101010u8);
let walking_ones: Bits<U8> = bits(0b00000001u8);

// Maximum values for bit widths
let max4: Bits<U4> = bits(15u8);      // 2^4 - 1
let max8: Bits<U8> = bits(255u8);     // 2^8 - 1  
let max16: Bits<U16> = bits(65535u16); // 2^16 - 1
```

## Shift Operations

RHDL supports both left shift (`<<`) and right shift (`>>`) operations on `Bits<N>`:

### Left Shift (`<<`)
```rust
let val: Bits<U8> = bits(0b00001111u8); // 0x0F

// Shift by literal amount
let shifted: Bits<U8> = val << 2;        // 0b00111100 = 0x3C
let shifted: Bits<U8> = val << 4u128;    // 0b11110000 = 0xF0

// Shift by another Bits value
let shift_amount: Bits<U3> = bits(3u8);
let shifted: Bits<U8> = val << shift_amount; // 0b01111000 = 0x78

// Chained shifts
let result: Bits<U8> = val << 1 << 1;    // Same as << 2
```

### Right Shift (`>>`)
```rust
let val: Bits<U8> = bits(0b11110000u8); // 0xF0

// Shift by literal amount  
let shifted: Bits<U8> = val >> 2;        // 0b00111100 = 0x3C
let shifted: Bits<U8> = val >> 4u128;    // 0b00001111 = 0x0F

// Shift by another Bits value
let shift_amount: Bits<U3> = bits(3u8);
let shifted: Bits<U8> = val >> shift_amount; // 0b00011110 = 0x1E
```

### Shift Properties
```rust
// Shifts are logical (not arithmetic) - zeros fill in
let val: Bits<U8> = bits(0b10110100u8);
let left: Bits<U8> = val << 1;   // 0b01101000 (zero fills from right)  
let right: Bits<U8> = val >> 1;  // 0b01011010 (zero fills from left)

// Shifting by >= bit width gives zero
let val: Bits<U8> = bits(0xFFu8);
let zero: Bits<U8> = val << 8;   // All zeros (shifted out completely)
let zero: Bits<U8> = val >> 8;   // All zeros

// Wrapping behavior for large shifts
let val: Bits<U4> = bits(0xFu8);
let result: Bits<U4> = val << 100; // Same as val << (100 % bit_width)
```

### Shift Assignment Operators
```rust
let mut val: Bits<U8> = bits(0x0Fu8);

// Left shift assignment
val <<= 2;              // val is now 0x3C
val <<= bits(1u8);      // val is now 0x78

// Right shift assignment  
val >>= 3;              // val is now 0x0F
val >>= bits(2u8);      // val is now 0x03
```

### Common Shift Patterns
```rust
// Power of 2 multiplication/division
let val: Bits<U16> = bits(100u16);
let times_4: Bits<U16> = val << 2;   // Multiply by 4 (2^2)
let times_8: Bits<U16> = val << 3;   // Multiply by 8 (2^3)
let div_by_4: Bits<U16> = val >> 2;  // Divide by 4 (integer division)

// Bit isolation using shifts and masks
let data: Bits<U16> = bits(0x1234u16);
let middle_byte: Bits<U16> = (data << 4) >> 8; // Extract middle 8 bits

// Creating bit patterns
let pattern: Bits<U8> = bits(1u8) << 7;        // 0b10000000
let walking_bit: Bits<U8> = bits(1u8) << 3;    // 0b00001000
```

### Shifts in Kernel Functions
```rust
#[kernel]
fn barrel_shifter<N: BitWidth>(
    _cr: ClockReset,
    input: (Bits<N>, Bits<U8>, bool), // (data, shift_amount, left_shift)
    _q: ()
) -> (Bits<N>, ()) {
    let (data, shift_amount, left_shift) = input;
    
    let result = if left_shift {
        data << shift_amount
    } else {
        data >> shift_amount  
    };
    
    (result, ())
}

#[kernel] 
fn multiply_by_constant<N: BitWidth>(
    _cr: ClockReset,
    input: Bits<N>,
    _q: ()
) -> (Bits<N>, ()) {
    // Multiply by 5 using shifts and addition: x * 5 = (x << 2) + x
    let times_4 = input << 2;
    let times_5 = times_4 + input;
    
    (times_5, ())
}
```

## Slicing Bits<N>

**Important**: `Bits<N>` does **not** provide slice methods directly. Instead, RHDL uses standalone functions from the `rhdl-std` crate for slicing operations.

### Why No Built-in Slice Methods?

RHDL intentionally separates slicing operations from the core `Bits<N>` type for several hardware-focused reasons:

1. **Compile-time Type Safety**: Slicing functions require compile-time knowledge of both input and output bit widths for efficient hardware generation
2. **Hardware Synthesis**: Each slice operation generates specialized Verilog code optimized for specific bit widths
3. **External Kernel Generation**: Functions like `slice::<8, 4>()` map to specific hardware primitives
4. **Type System Enforcement**: The function signature `slice<const N: usize, const M: usize>(x: Bits<N>, start: u128) -> Bits<M>` ensures the output type exactly matches the requested slice width

### Importing Slicing Functions
```rust
use rhdl::prelude::*;
use rhdl_std::*; // Import slicing functions
```

### Range Slicing with `slice()`
```rust
use rhdl_std::slice; // Import the slice function

let val: Bits<U16> = bits(0xABCDu16);

// Extract bit ranges using slice::<INPUT_WIDTH, OUTPUT_WIDTH>(bits, start)
let upper_byte: Bits<U8> = slice::<16, 8>(val, 8);   // 8 bits starting at position 8 = 0xAB
let lower_byte: Bits<U8> = slice::<16, 8>(val, 0);   // 8 bits starting at position 0 = 0xCD  
let middle_nibble: Bits<U4> = slice::<16, 4>(val, 8); // 4 bits starting at position 8 = 0xC
```

### Single Bit Extraction  
```rust
use rhdl_std::get_bit; // Import the get_bit function

let val: Bits<U8> = bits(0b10110100u8);

// Extract individual bits (0-indexed from LSB)
let bit0: bool = get_bit(val, 0); // LSB = false (0)
let bit7: bool = get_bit(val, 7); // MSB = true (1) 
let bit5: bool = get_bit(val, 5); // = true (1)
```

### Common Slicing Patterns
```rust
use rhdl_std::slice;

let data: Bits<U32> = bits(0x12345678u32);

// Extract bytes
let byte0: Bits<U8> = slice::<32, 8>(data, 0);  // 8 bits from pos 0 = 0x78 (LSB)
let byte1: Bits<U8> = slice::<32, 8>(data, 8);  // 8 bits from pos 8 = 0x56
let byte2: Bits<U8> = slice::<32, 8>(data, 16); // 8 bits from pos 16 = 0x34
let byte3: Bits<U8> = slice::<32, 8>(data, 24); // 8 bits from pos 24 = 0x12 (MSB)

// Extract nibbles (4-bit chunks)
let nibble0: Bits<U4> = slice::<32, 4>(data, 0);  // 0x8
let nibble1: Bits<U4> = slice::<32, 4>(data, 4);  // 0x7
let nibble2: Bits<U4> = slice::<32, 4>(data, 8);  // 0x6

// Extract arbitrary ranges
let middle_12_bits: Bits<U12> = slice::<32, 12>(data, 8); // 12 bits starting at position 8
```

### Hardware-Optimized Slicing Functions

The `rhdl-fpga` crate provides additional hardware-optimized functions:

```rust
use rhdl_fpga::core::slice::{lsbs, msbs};

let data: Bits<U32> = bits(0xDEADBEEFu32);

// Extract N least significant bits
let lower_16: Bits<U16> = lsbs::<U16, U32>(data); // 0xBEEF

// Extract N most significant bits  
let upper_16: Bits<U16> = msbs::<U16, U32>(data); // 0xDEAD

// These functions use compile-time loops for efficient hardware generation
```

### Slicing in Hardware Contexts
```rust
use rhdl_std::{slice, get_bit};

#[kernel]
fn extract_fields(
    _cr: ClockReset,
    input: Bits<U16>,
    _q: ()
) -> ((Bits<U4>, Bits<U8>, bool), ()) {
    // Extract different fields from input
    let field1: Bits<U4> = slice::<16, 4>(input, 0);    // Lower 4 bits
    let field2: Bits<U8> = slice::<16, 8>(input, 4);    // Middle 8 bits (positions 4-11)
    let flag: bool = get_bit(input, 15);                // Single bit flag
    
    ((field1, field2, flag), ())
}

// Each slice operation generates optimized Verilog like:
// function [3:0] slice_16_4(input [15:0] a, input integer start);
//     slice_16_4 = a[start+:4];
// endfunction
```

## Complete Example: Packet Processing

```rust
use rhdl::prelude::*;

#[derive(Digital, Clone, Copy, PartialEq)]
struct Packet {
    header: Bits<U8>,
    payload: Bits<U16>, 
    checksum: Bits<U8>,
}

#[kernel]
fn build_packet(
    _cr: ClockReset,
    input: (Bits<U8>, Bits<U16>, Bits<U8>), // (header, payload, checksum)
    _q: ()
) -> (Bits<U32>, ()) {
    let (header, payload, checksum) = input;
    
    // Concatenate fields into 32-bit packet manually
    let packet_bits = bits((header.val << 24) | (payload.val << 8) | checksum.val);
    
    (packet_bits, ())
}

#[kernel]  
fn parse_packet(
    _cr: ClockReset,
    input: Bits<U32>,
    _q: ()
) -> (Packet, ()) {
    // Extract fields from 32-bit packet
    let header: Bits<U8> = slice::<32, 8>(input, 24);   // Upper 8 bits
    let payload: Bits<U16> = slice::<32, 16>(input, 8); // Middle 16 bits
    let checksum: Bits<U8> = slice::<32, 8>(input, 0);  // Lower 8 bits
    
    let packet = Packet { header, payload, checksum };
    (packet, ())
}

// Usage example
fn main() -> Result<(), RHDLError> {
    // Create packet
    let header = bits(0xAAu8);
    let payload = bits(0x1234u16); 
    let checksum = bits(0x55u8);
    
    let (packet_bits, _) = build_packet(ClockReset::default(), (header, payload, checksum), ());
    println!("Packet bits: 0x{:08X}", packet_bits.val); // 0xAA123455
    
    // Parse packet back
    let (parsed, _) = parse_packet(ClockReset::default(), packet_bits, ());
    println!("Header: 0x{:02X}", parsed.header.val);   // 0xAA
    println!("Payload: 0x{:04X}", parsed.payload.val); // 0x1234  
    println!("Checksum: 0x{:02X}", parsed.checksum.val); // 0x55
    
    Ok(())
}
```

## Key Takeaways

1. **Creation**: Use `bits(literal)` with explicit types for bit width safety
2. **Concatenation**: No built-in function due to Rust limitations - use manual approach: `bits((high.val << LOW_WIDTH) | low.val)`  
3. **Slicing**: Import `rhdl_std::slice` and use `slice::<INPUT_WIDTH, OUTPUT_WIDTH>(bits, start)`
4. **Single bits**: Import `rhdl_std::get_bit` and use `get_bit(bits, index)`
5. **Type safety**: All operations are checked at compile time with explicit width parameters
6. **Hardware friendly**: All operations synthesize to efficient, specialized hardware primitives
7. **Design philosophy**: RHDL separates basic operations (`Bits<N>`) from complex operations (`rhdl-std`) for optimal hardware synthesis
8. **Language limitations**: RHDL cannot provide `cat()` until Rust stabilizes `generic_const_exprs`

These operations form the foundation for all bit manipulation in RHDL circuits!