# Core RHDL Traits: A Complete Guide

RHDL has several fundamental traits that work together to enable hardware design in Rust. Understanding these traits and their relationships is essential for effective RHDL usage.

## Trait Hierarchy Overview

```
Digital (foundation)
    ↓
Timed (adds timing semantics)  
    ↓
BitWidth (compile-time bit width)
    ↓
SynchronousDQ (state interface)
    ↓  
SynchronousIO (circuit interface)
    ↓
Synchronous (complete synchronous circuit)

Circuit (async alternative to Synchronous)
```

## 1. Digital - The Foundation

**Purpose**: Marks types that can be converted to hardware bits.

```rust
pub trait Digital: Copy + PartialEq + Sized + Clone + 'static {
    const BITS: usize;                    // Bit width requirement
    fn static_kind() -> Kind;             // Type metadata  
    fn bin(self) -> Vec<BitX>;            // Convert to bits
    fn dont_care() -> Self;               // Reset value
}
```

**Who implements it**: `bool`, `u8/u16/u32/u64/u128`, `i8/i16/i32/i64/i128`, `Bits<N>`, `SignedBits<N>`, tuples, arrays, `Option<T>`, `Result<T,E>`, custom structs/enums.

**Key insight**: If it's not `Digital`, it can't be used in hardware.

## 2. BitWidth - Compile-Time Bit Widths  

**Purpose**: Represents compile-time known bit widths using type-level numbers.

```rust
pub trait BitWidth: Copy + Clone + Default + PartialEq + Eq + 'static {
    const BITS: usize;
}
```

**Usage**:
```rust
// Type-level numbers from typenum crate
U1, U2, U3, U4, U8, U16, U32, U64, U128 // etc.

// Used in generic contexts
Bits<U8>        // 8-bit vector
Counter<U4>     // 4-bit counter  
ShiftRegister<U16> // 16-bit shift register
```

**Why it exists**: Enables compile-time verification of bit widths and generic hardware components.

## 3. Timed - Digital with Timing Semantics

**Purpose**: Extends `Digital` for types that can be used in timed simulations.

```rust
pub trait Timed: Digital {
    fn static_kind() -> Kind {
        <Self as Digital>::static_kind()
    }
}
```

**Who implements it**: Most `Digital` types automatically implement `Timed`.

**Usage**: Required for `Circuit` inputs/outputs (asynchronous circuits).

## 4. SynchronousDQ - State Interface

**Purpose**: Defines the internal state types for synchronous components (D = data in, Q = data out).

```rust
pub trait SynchronousDQ: 'static + Sized + Clone {
    type D: Digital;  // Next state data type
    type Q: Digital;  // Current state data type  
}
```

**Examples**:
```rust
impl<T: Digital> SynchronousDQ for DFF<T> {
    type D = ();  // DFF manages its own state
    type Q = ();  
}

// Counter's derive macro generates:
// type D = D<N> { count: Bits<N> }
// type Q = Q<N> { count: Bits<N> }
```

## 5. SynchronousIO - Circuit Interface

**Purpose**: Defines the external interface and behavior of synchronous components.

```rust
pub trait SynchronousIO: SynchronousDQ {
    type I: Digital;     // Input type
    type O: Digital;     // Output type  
    type Kernel: DigitalFn3<
        A0 = ClockReset, 
        A1 = Self::I, 
        A2 = Self::Q, 
        O = (Self::O, Self::D)
    >;
}
```

**Key constraint**: The `Kernel` type **must** be a function with signature:
```rust
fn kernel(cr: ClockReset, input: I, state: Q) -> (O, D)
```

**Examples**:
```rust
impl<N: BitWidth> SynchronousIO for Counter<N> {
    type I = bool;              // Enable signal
    type O = Bits<N>;           // Count output
    type Kernel = counter<N>;   // References kernel function
}
```

## 6. Synchronous - Complete Synchronous Circuit

**Purpose**: The full trait for synchronous digital circuits.

```rust  
pub trait Synchronous: SynchronousIO {
    type S: PartialEq + Clone;  // Simulation state

    fn init(&self) -> Self::S;  // Initial state
    fn sim(&self, clock_reset: ClockReset, input: Self::I, state: &mut Self::S) -> Self::O;
    fn description(&self) -> String; // Documentation
    fn hdl(&self, name: &str) -> Result<HDLDescriptor, RHDLError>; // Hardware generation
}
```

**Usually derived**: Most users derive this rather than implement manually:
```rust
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
pub struct MyCircuit<N: BitWidth> {
    // fields become internal state automatically
}
```

## 7. Circuit Family (Asynchronous Alternative)

For asynchronous (combinational) circuits:

```rust
pub trait CircuitDQ: 'static + Sized + Clone {
    type D: Timed;  // Uses Timed instead of Digital
    type Q: Timed;  
}

pub trait CircuitIO: CircuitDQ {
    type I: Timed;
    type O: Timed;
    type Kernel: DigitalFn2<A0 = Self::I, A1 = Self::Q, O = (Self::O, Self::D)>;
    // Note: No ClockReset parameter!
}

pub trait Circuit: CircuitIO {
    type S: Clone + PartialEq;
    // Similar to Synchronous but for async circuits
}
```

## Trait Usage Patterns

### For Basic Hardware Types
```rust  
#[derive(Digital, Clone, Copy, PartialEq)]
struct MyData {
    flag: bool,
    value: Bits<U8>,
}
```

### For Synchronous Components
```rust
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]  
pub struct MyProcessor<N: BitWidth> {
    register_file: [dff::DFF<Bits<N>>; 8],
    pc: dff::DFF<Bits<N>>,
}

impl<N: BitWidth> SynchronousIO for MyProcessor<N> {
    type I = (bool, Bits<N>);    // (enable, instruction)
    type O = Bits<N>;            // result
    type Kernel = my_processor_kernel<N>;
}

#[kernel]
fn my_processor_kernel<N: BitWidth>(
    cr: ClockReset, 
    input: (bool, Bits<N>), 
    q: Q<N>
) -> (Bits<N>, D<N>) {
    // Implementation
}
```

### For Custom Data Types  
```rust
#[derive(Digital, Clone, Copy, PartialEq)]
enum Opcode {
    Add,
    Sub, 
    Mul,
    Nop,
}

#[derive(Digital, Clone, Copy, PartialEq)]  
struct Instruction {
    opcode: Opcode,
    src1: Bits<U4>,
    src2: Bits<U4>, 
    dst: Bits<U4>,
}
```

## Key Relationships

1. **Digital → Everything**: All hardware types must be `Digital`
2. **BitWidth → Generics**: Enables parameterized bit widths  
3. **SynchronousDQ → State**: Defines how state flows through circuits
4. **SynchronousIO → Interface**: Defines external behavior and kernel function
5. **Synchronous → Complete**: Adds simulation and hardware generation

## Common Derive Combinations

```rust
// Basic data type
#[derive(Digital, Clone, Copy, PartialEq)]

// Synchronous component  
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]

// With additional traits for debugging/testing
#[derive(Digital, Clone, Copy, PartialEq, Debug, Default)]
```

## Essential Takeaways

1. **Digital is foundational** - nothing works in RHDL without it
2. **BitWidth enables generics** - parameterized hardware components
3. **SynchronousDQ/IO/Synchronous form a hierarchy** - each builds on the previous
4. **Derive macros do most of the work** - manual implementation rarely needed
5. **Kernel functions must match strict signatures** - enforced by trait bounds
6. **Circuit traits are the async alternative** - for combinational logic

These traits work together to provide **compile-time guarantees** that your hardware designs are correct and synthesizable, while maintaining the expressiveness and safety of Rust's type system.