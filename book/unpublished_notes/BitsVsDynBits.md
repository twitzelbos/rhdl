# Bits vs DynBits: Compile-Time vs Runtime Bit Vectors

RHDL provides two different bit vector types that serve distinct purposes in hardware design and tooling. Understanding their differences is crucial for effective RHDL usage.

## Structure Comparison

### Bits<N> - Compile-Time Bit Width
```rust
pub struct Bits<Len> {
    pub(crate) marker: std::marker::PhantomData<Len>,  // Type-level size
    pub val: u128,                                     // Value storage
}
```

### DynBits - Runtime Bit Width
```rust
pub struct DynBits {
    pub val: u128,       // Value storage  
    pub bits: usize,     // Runtime bit width!
}
```

## Key Differences

| Feature | Bits<N> | DynBits |
|---------|---------|---------|
| **Bit width** | Compile-time constant | Runtime variable |
| **Type safety** | Different widths = different types | Same type regardless of width |
| **Runtime overhead** | Zero (no width storage) | Extra `usize` field |
| **Digital trait** | ✅ Implements Digital | ❌ Cannot implement Digital |
| **Hardware synthesis** | ✅ Synthesizable | ❌ Not synthesizable |
| **Usage** | Circuits, kernels, hardware | Tools, debugging, utilities |

## The Digital Trait Constraint

**Why DynBits cannot implement Digital:**

```rust
pub trait Digital: Copy + PartialEq + Sized + Clone + 'static {
    const BITS: usize;  // ❌ Must be compile-time constant!
    // ...
}
```

`DynBits` has runtime bit width, but `Digital` requires `const BITS: usize` - a compile-time constant. This fundamental mismatch means **DynBits cannot be Digital**.

## What This Means in Practice

### ❌ DynBits CANNOT be used for:

```rust
// Synchronous circuits
struct BadCounter {
    count: dff::DFF<DynBits>,  // ❌ Compile error - not Digital
}

// Kernel functions
#[kernel]  
fn bad_kernel(cr: ClockReset, input: DynBits) -> DynBits {  // ❌ Not Digital
    // Won't compile
}

// Circuit interfaces
impl SynchronousIO for Something {
    type I = DynBits;  // ❌ Must implement Digital
    type O = DynBits;  // ❌ Must implement Digital
}
```

### ✅ DynBits CAN be used for:

```rust
// Runtime bit manipulation
let width = calculate_width_at_runtime();
let value = DynBits { val: 0xABCD, bits: width };

// Debugging and analysis tools
fn debug_signal(signal: DynBits) {
    println!("Signal: {} bits, value: 0x{:x}", signal.bits, signal.val);
}

// Simulation utilities  
fn analyze_trace(signals: Vec<DynBits>) {
    // Process signals with different widths
}

// Manual conversion from fixed width
let fixed: Bits<U8> = bits(0xFF);
let dynamic = DynBits { val: fixed.val, bits: 8 };
```

## When to Use Each

### Use **Bits<N>** when:
- **Building hardware circuits** - counters, shift registers, processors
- **Kernel functions** - synchronous logic implementation  
- **Type safety matters** - want compile-time width verification
- **Performance critical** - no runtime overhead needed
- **Fixed, known widths** - bit width determined at design time

```rust
// Hardware synthesis
#[derive(Synchronous, SynchronousDQ)]
pub struct Processor<N: BitWidth> {
    registers: [dff::DFF<Bits<N>>; 16],  // ✅ Fixed width
    pc: dff::DFF<Bits<N>>,
}
```

### Use **DynBits** when:
- **Simulation and debugging** - analyzing signals of various widths
- **Development tools** - waveform viewers, signal analyzers  
- **Runtime operations** - bit width not known until runtime
- **Interfacing with external tools** - that don't have compile-time types
- **Utility functions** - that need to work with arbitrary widths

```rust
// Debugging/analysis tool
fn generate_vcd_trace(signals: &[(String, Vec<DynBits>)]) {
    for (name, values) in signals {
        // Can handle any bit width at runtime
        for value in values {
            println!("{}: {}'h{:x}", name, value.bits, value.val);
        }
    }
}
```

## Design Philosophy

This separation reflects RHDL's **"hardware-first"** design philosophy:

### Compile-Time Safety
- `Bits<N>` enforces hardware constraints at compile time
- Prevents non-synthesizable designs from being written
- Type system catches width mismatches before hardware generation

### Runtime Flexibility  
- `DynBits` provides escape hatch for tooling and utilities
- Enables dynamic operations not possible with fixed types
- Supports development and debugging workflows

## Type System Enforcement

```rust
// Compile-time verification
let a: Bits<U8> = bits(0xFF);
let b: Bits<U16> = bits(0xABCD);
// let c = a + b;  // ❌ Compile error - incompatible types!

// Runtime flexibility
let x = DynBits { val: 0xFF, bits: 8 };
let y = DynBits { val: 0xABCD, bits: 16 };
// Can operate on different widths (with runtime checks)
```

## Key Takeaways

1. **Hardware → Bits<N>**: If it's going to synthesized hardware, use `Bits<N>`
2. **Software → DynBits**: If it's for tooling/debugging/utilities, use `DynBits`  
3. **Digital trait is the boundary**: Only `Digital` types can be used in circuits
4. **Compile-time vs Runtime**: Choose based on when bit width is known
5. **Type safety**: RHDL prevents runtime bit-width types in hardware contexts

## The Bottom Line

**`Bits<N>` = Hardware-synthesizable, type-safe, zero-overhead**  
**`DynBits` = Flexible, runtime-determined, tooling-focused**

This design ensures that RHDL catches non-synthesizable designs at **compile time** rather than after expensive hardware generation, while still providing the flexibility needed for development tools and utilities.