# Understanding RHDL's Synchronous Traits

This document explains RHDL's core traits for building synchronous digital circuits in plain terms.

## SynchronousDQ: The Storage Interface

The `SynchronousDQ` trait defines what types of data flow into and out of a component's internal storage elements. The name comes from classical digital logic terminology:

- **D** = Data input (what goes into flip-flops on the next clock edge)
- **Q** = Output (what comes out of flip-flops - the current stored value)

### Important Limitation

⚠️ **Only one `SynchronousDQ` derive per file/module**: The `#[derive(SynchronousDQ)]` macro creates global type aliases `Q` and `D` in the current scope. Having multiple `SynchronousDQ` derives in the same file will cause naming conflicts.

**Workaround**: If you need multiple synchronous components in the same file, define them in separate modules or files.

### Definition
```rust
pub trait SynchronousDQ: 'static + Sized + Clone {
    type D: Digital;  // Next state data type
    type Q: Digital;  // Current state data type  
}
```

### What It Means
Think of it like defining the "memory interface" for your component:
- `type D`: The shape of data that gets stored on each clock cycle
- `type Q`: The shape of data that can be read from storage

### Examples

**Simple case (DFF):**
```rust
impl<T: Digital> SynchronousDQ for DFF<T> {
    type D = ();  // No exposed internal state
    type Q = ();  // DFF manages its own D/Q internally
}
```

**Complex case (Counter with internal state):**
The `Counter` uses a derive macro that automatically generates D/Q types based on its fields.

## SynchronousIO: The Circuit Interface  

The `SynchronousIO` trait defines the external interface of a synchronous component - what signals come in and go out, like the pinout of an IC.

### Definition
```rust
pub trait SynchronousIO: SynchronousDQ {
    type I: Digital;     // Input signal type
    type O: Digital;     // Output signal type
    type Kernel: /* Complex constraint requiring ClockReset first */;
}
```

### What It Means
This is like the "datasheet" for your digital component:
- `type I`: What type of signal comes into this component
- `type O`: What type of signal this component produces
- `type Kernel`: The function that implements the component's behavior

### The Hidden Requirement
The `Kernel` type has a complex constraint that **requires** the implementing function to have this exact signature:
```rust
fn kernel_function(cr: ClockReset, input: I, state: Q) -> (O, D)
```

This means:
1. **First parameter must be `ClockReset`** - every synchronous component needs clock/reset
2. **Second parameter is the input** (`type I`)  
3. **Third parameter is current state** (`type Q` from SynchronousDQ)
4. **Returns both output and next state** (`type O` and `type D`)

### Counter Example
```rust
impl<N: BitWidth> SynchronousIO for Counter<N> {
    type I = bool;           // Takes a boolean enable signal
    type O = Bits<N>;        // Outputs N-bit count value
    type Kernel = counter<N>; // Uses the counter kernel function
}

#[kernel]
pub fn counter<N: BitWidth>(cr: ClockReset, enable: bool, q: Q<N>) -> (Bits<N>, D<N>) {
    let next_count = if enable { q.count + 1 } else { q.count };
    let next_count = if cr.reset.any() { bits(0) } else { next_count };
    (q.count, D::<N> { count: next_count })
}
```

## Why These Traits Exist

### Hardware Design Patterns
These traits enforce well-established digital design principles:

1. **Separation of Combinational and Sequential Logic**: The kernel function describes pure combinational logic, while the framework handles the sequential (clocked) behavior.

2. **Moore/Mealy Machine Pattern**: 
   - Inputs + Current State → Next State + Outputs
   - This is the fundamental pattern of all synchronous digital systems

3. **Clock Domain Discipline**: By requiring `ClockReset` as the first parameter, RHDL ensures all components in a circuit share the same timing reference.

### Type Safety Benefits
- **Interface Matching**: Can't connect incompatible signal types
- **State Consistency**: D and Q types must align across the design
- **Timing Correctness**: All synchronous components guaranteed to have proper clock/reset handling

## Real-World Analogy

Think of these traits like **standardized IC packages**:

- `SynchronousDQ` = The internal storage specification (like SRAM interface)
- `SynchronousIO` = The external pin configuration (like a 74-series logic IC pinout)
- The kernel function = The truth table/behavior specification

Just as you can't connect a 5V output to a 3.3V input in hardware, RHDL's type system prevents you from connecting incompatible digital signal types in your design.

## Key Takeaway

These traits provide **compile-time guarantees** that your synchronous digital circuit:
1. Has proper clock/reset distribution
2. Has type-safe signal connections  
3. Follows established digital design patterns
4. Can be automatically converted to hardware (Verilog/VHDL)

They're RHDL's way of making "correct by construction" digital designs.