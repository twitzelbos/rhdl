# Understanding RHDL's Digital Trait

The `Digital` trait is **RHDL's core abstraction** for any data type that can be represented in hardware. It serves as the "synthesizable" marker trait that defines the contract between Rust types and hardware generation.

## What Does Digital Mean?

**Digital = "Can be converted to hardware"**

If a type implements `Digital`, it can be:
- Passed through synchronous circuits
- Converted to Verilog/VHDL
- Traced in simulations  
- Stored in registers/memory
- Used in kernel functions

## Trait Definition

```rust
pub trait Digital: Copy + PartialEq + Sized + Clone + 'static {
    const BITS: usize;                    // How many bits this type needs
    fn static_kind() -> Kind;             // Type metadata for hardware generation
    fn bin(self) -> Vec<BitX>;            // Convert to raw bits  
    fn dont_care() -> Self;               // Default/reset value
    
    // Additional methods for tracing and debugging
    fn static_trace_type() -> TraceType;
    fn typed_bits(self) -> TypedBits;
    fn binary_string(self) -> String;
    // ...
}
```

## Core Requirements

### 1. Bit Width (`BITS`)
Every `Digital` type must declare exactly how many hardware bits it requires:
```rust
impl Digital for bool {
    const BITS: usize = 1;  // A bool needs 1 bit
}

impl Digital for u8 {
    const BITS: usize = 8;  // A u8 needs 8 bits  
}

impl<N: BitWidth> Digital for Bits<N> {
    const BITS: usize = N::BITS;  // Bits<N> needs N bits
}
```

### 2. Bit Conversion (`bin()`)
Must be able to convert the value to a vector of hardware bits:
```rust
impl Digital for bool {
    fn bin(self) -> Vec<BitX> {
        vec![if self { BitX::One } else { BitX::Zero }]
    }
}
```

### 3. Type Metadata (`static_kind()`)
Provides information needed for hardware generation:
```rust
impl Digital for bool {
    fn static_kind() -> Kind {
        Kind::make_bits(1)  // This is a 1-bit type
    }
}
```

### 4. Reset Values (`dont_care()`)
Provides a default value for reset conditions:
```rust
impl Digital for bool {
    fn dont_care() -> Self {
        false  // Reset value for bool
    }
}
```

## Built-in Digital Types

RHDL automatically implements `Digital` for:

### Primitive Types
- `bool` → 1 bit
- `u8`, `i8` → 8 bits  
- `u16`, `i16` → 16 bits
- `u32`, `i32` → 32 bits
- `u64`, `i64` → 64 bits
- `u128`, `i128` → 128 bits
- `usize` → platform-dependent bits

### RHDL-Specific Types
- `Bits<N>` → N bits (unsigned bit vector)
- `SignedBits<N>` → N bits (signed bit vector)
- `()` → 0 bits (unit type)

### Compound Types
If the components implement `Digital`, these do too:
- `Option<T>` → discriminant + T bits
- `Result<T, E>` → discriminant + max(T, E) bits  
- `(T, U)` → T + U bits (tuples)
- `[T; N]` → N × T bits (arrays)
- Custom structs and enums (with derive macros)

## Examples

### Basic Usage
```rust
// Check bit requirements
assert_eq!(bool::BITS, 1);
assert_eq!(u32::BITS, 32);
assert_eq!(Bits::<U8>::BITS, 8);

// Convert to bits
let value = true;
assert_eq!(value.bin(), vec![BitX::One]);

let byte_value = 0xABu8;
assert_eq!(byte_value.bin().len(), 8);
```

### Compound Types
```rust
// Tuples combine bit requirements
type MyTuple = (bool, u8, Bits<U4>);
assert_eq!(MyTuple::BITS, 1 + 8 + 4); // 13 bits total

// Options add discriminant overhead
type OptionalByte = Option<u8>;  
// Needs more than 8 bits due to Some/None discriminant
```

### Custom Types
```rust
#[derive(Digital, Clone, Copy, PartialEq)]
struct MyStruct {
    flag: bool,
    counter: Bits<U4>,
    data: u8,
}

// Automatically implements Digital with BITS = 1 + 4 + 8 = 13
```

## Why Digital Exists

### Type Safety at Hardware Boundary
The `Digital` trait prevents you from accidentally using non-synthesizable types in hardware contexts:

```rust
// ✅ This works - bool implements Digital
let counter: Counter<U4> = Counter::default();
let input: bool = true;

// ❌ This won't compile - String doesn't implement Digital  
// let bad_input: String = "hello".to_string();
```

### Hardware Generation
RHDL uses `Digital` information to:
- Calculate register widths in generated Verilog
- Create proper port declarations
- Generate reset logic
- Optimize hardware layouts

### Simulation and Debugging
The trait provides methods for:
- Converting values to bit patterns for VCD traces
- Displaying binary representations  
- Creating "don't care" values for uninitialized state

## Real-World Analogy

Think of `Digital` like **electrical specifications** for electronic components:
- Just as every component has voltage/current requirements
- Every RHDL type has bit width and conversion requirements
- Just as you can't connect incompatible voltages
- You can't use non-Digital types in hardware circuits

## Key Takeaway

The `Digital` trait is RHDL's **contract for hardware compatibility**. It ensures that every type used in synchronous circuits:
1. Has a well-defined bit representation
2. Can be converted to/from hardware bits
3. Has appropriate reset values
4. Provides metadata for hardware generation

This enables RHDL to provide **compile-time guarantees** that your circuit designs will translate correctly to actual hardware.