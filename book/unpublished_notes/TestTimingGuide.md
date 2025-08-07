# RHDL Test Timing Guide

This document explains how to properly time your RHDL synchronous circuit tests using the `with_reset()` and `clock_pos_edge()` methods.

## Overview

RHDL provides a realistic test framework that simulates actual digital circuit timing behavior. Understanding how these timing methods work is crucial for writing correct tests for synchronous circuits.

## Test Data Generation Pattern

Most RHDL tests follow this pattern:

```rust
let input = test_data_stream
    .with_reset(1)           // Hold reset for 1 cycle
    .clock_pos_edge(100);    // 100 time unit clock period
```

## `with_reset(n)` Method

### Purpose
Generates a proper reset sequence at the beginning of your test, simulating how real digital circuits start up.

### Parameters
- `n`: Number of clock cycles to hold reset active

### Behavior
1. **Reset Phase**: For the first `n` clock cycles, the circuit is held in reset state
   - Reset signal = `true`
   - All registers return to their default/reset values
   - Circuit outputs reflect the reset state

2. **Normal Operation**: After `n` cycles, reset is released
   - Reset signal = `false`
   - Circuit begins processing your test data normally

### Common Values
```rust
.with_reset(1)    // Most common - single reset cycle
.with_reset(2)    // Longer reset for complex circuits  
.with_reset(4)    // Extended reset (used by counter tests)
```

### Example
```rust
let test_data = vec![bits(0xAB), bits(0x56), bits(0xCD)];
let input = test_data.with_reset(1).clock_pos_edge(100);

// Timeline:
// Cycle 0: RESET - circuit in reset state, outputs at default values
// Cycle 1: Process bits(0xAB) 
// Cycle 2: Process bits(0x56)
// Cycle 3: Process bits(0xCD)
```

## `clock_pos_edge(frequency)` Method

### Purpose
Generates realistic clock timing with proper low/high transitions, simulating how synchronous circuits operate with actual clock signals.

### Parameters
- `frequency`: Clock period in time units (NOT frequency in Hz)
  - Higher values = slower clock
  - `100` is commonly used for most tests

### Behavior
Creates a complete clock waveform for each input:

1. **Clock Low Phase**: Clock signal is low, data is stable
2. **Positive Edge**: Clock transitions from low to high
   - This is when synchronous circuits sample inputs
   - Register updates occur on this edge
3. **Clock High Phase**: Clock signal is high, data may change
4. **Clock Low Phase**: Clock returns low, completing the cycle

### Multiple Samples Per Input
**Documented Behavior**: Based on the actual RHDL source code (`/rhdl/src/rhdl_core/sim/clock_pos_edge.rs`), each input data value generates **exactly 3 time samples** through a state machine:

1. **Clock Low Sample**: Clock = false, data = current input
2. **Clock High Sample**: Clock = true (positive edge), data = current input  
3. **Clock High with Next Data**: Clock = true, data = next input (sampled on edge)

```rust
// Input: [A, B, C]
// Generated samples: [A, A, A, B, B, B, C, C, C]
//                    └─────┘ └─────┘ └─────┘
//                    Clock   Clock   Clock
//                    Cycle 1 Cycle 2 Cycle 3
```

### Example
```rust
let input = vec![bits(0x1), bits(0x2)].with_reset(0).clock_pos_edge(10);

// Timeline (period = 10 time units):
// Time 0-4:   Clock low,  data = 0x1
// Time 5:     Clock high (positive edge), circuits sample 0x1  
// Time 6-9:   Clock high, data may transition to 0x2
// Time 10-14: Clock low,  data = 0x2
// Time 15:    Clock high (positive edge), circuits sample 0x2
// Time 16-19: Clock high, data stable
```

## Understanding Test Output Timing

### Why You See Repeated Values
When you collect test outputs, you'll see patterns like:
```rust
[false, false, false, true, true, true, false, false, false, ...]
```

This is **normal and expected**! Each group of identical values represents one input being sampled across multiple clock phases.

### Timing Calculation
For `with_reset(n).clock_pos_edge(period)`:

1. **Reset cycles**: First `n × 3` samples will show reset behavior
2. **Data cycles**: Each input produces exactly 3 samples  
3. **Total samples**: Exactly `(n + input_count) × 3`

**Source**: This behavior is implemented in the `ClockPosEdge` state machine in `/rhdl/src/rhdl_core/sim/clock_pos_edge.rs`

### Example Timing Analysis
```rust
let test_data = vec![bits(0xAB), bits(0x56)]; // 2 inputs
let input = test_data.with_reset(1).clock_pos_edge(100);
let outputs: Vec<_> = circuit.run(input)?.collect();

// Expected pattern:
// Indices 0-2:   Reset cycle (reset=true) - exactly 3 samples
// Indices 3-5:   Processing bits(0xAB) - exactly 3 samples
// Indices 6-8:   Processing bits(0x56) - exactly 3 samples  
// Total: exactly 9 samples

println!("{:?}", outputs);
// Output: [reset_val, reset_val, reset_val, out1, out1, out1, out2, out2, out2]
```

## Writing Correct Test Assertions

### **RECOMMENDED**: Pattern 1 - Use `synchronous_sample()`
The cleanest approach is to use `.synchronous_sample()` which automatically samples only at positive clock edges:

```rust
let input = test_data.with_reset(1).clock_pos_edge(100);
let outputs: Vec<_> = uut.run(input)?.synchronous_sample().map(|t| t.value.2).collect();

// Clean 1-sample-per-cycle: each index corresponds to one clock cycle
let expected = vec![
    false,  // Reset cycle 1 - output false
    false,  // Reset cycle 2 - output false (if with_reset(2))  
    true,   // After first input processed
    false,  // After second input processed  
    true,   // After third input processed
];

assert_eq!(outputs, expected);  // Simple, clean comparison!
```

**Benefits of `.synchronous_sample()`:**
- ✅ One sample per clock cycle - no need to account for 3x multiplier
- ✅ Directly corresponds to synchronous logic behavior
- ✅ Much simpler test assertions
- ✅ Matches patterns used throughout the RHDL codebase
- ✅ Eliminates timing calculation complexity

### Alternative Patterns (for raw output streams)

If you need the full timing detail, use these patterns with raw `.run()` output:

### Pattern 2: Test Specific Clock Cycles
```rust
// Test the output after each operation takes effect
assert_eq!(outputs[3], expected_after_input1);  // First occurrence of each phase
assert_eq!(outputs[6], expected_after_input2);  
```

### Pattern 3: Test Consistent Values Within Cycles
```rust
// Verify values are held consistently within each clock cycle
assert_eq!(outputs[3], outputs[4]);  // Should be identical
assert_eq!(outputs[4], outputs[5]);  // within the same cycle
```

### Pattern 4: Find Transition Points
```rust
// Find when outputs actually change
for (i, &output) in outputs.iter().enumerate() {
    if output == expected_value {
        println!("Expected output first appears at index {}", i);
        break;
    }
}
```

## Best Practices

### 1. **PREFERRED**: Use `.synchronous_sample()` for Clean Tests
```rust
// RECOMMENDED: Simple, clean 1-sample-per-cycle testing
let input = test_data.with_reset(1).clock_pos_edge(100);
let outputs: Vec<_> = uut.run(input)?.synchronous_sample().map(|t| t.value.2).collect();

// Test direct sequential logic behavior
assert_eq!(outputs[0], reset_output);      // Reset cycle
assert_eq!(outputs[1], after_input1);     // After first input
assert_eq!(outputs[2], after_input2);     // After second input
```

### 2. Use Consistent Reset Timing
```rust
// Prefer with_reset(1) for most tests - simpler timing
let input = test_data.with_reset(1).clock_pos_edge(100);
```

### 3. Alternative: Account for Multi-Sample Pattern (if using raw `.run()`)
```rust
// Only if you need full timing detail - use raw output
let outputs: Vec<_> = uut.run(input)?.map(|t| t.value.2).collect();

// Don't test every single sample - test the pattern
assert_eq!(outputs[4], true);   // First sample of the cycle
assert_eq!(outputs[5], true);   // Should be same
assert_eq!(outputs[6], true);   // Should be same
```

### 4. Debug Timing Issues
```rust
// When tests fail, print the actual timing pattern
println!("Output length: {}, pattern: {:?}", outputs.len(), &outputs[0..12]);

// For synchronous_sample debugging:
let sync_outputs: Vec<_> = uut.run(input)?.synchronous_sample().map(|t| t.value.2).collect();
println!("Synchronous samples: {:?}", sync_outputs);
```

### 5. Follow Existing Patterns
Look at working tests in the codebase:
```rust
// Most modern tests use synchronous_sample()
let outputs = uut.run(inputs)?.synchronous_sample();

// Legacy patterns (still valid)
let stream = rand_set.with_reset(4).clock_pos_edge(100);
let input = test_stream().with_reset(1).clock_pos_edge(100);
```

## Common Pitfalls

### ❌ Wrong: Using Raw Output Without Understanding Timing
```rust
// This assumes each input produces one output - INCORRECT with raw .run()
let outputs: Vec<_> = uut.run(input)?.map(|t| t.value.2).collect();
assert_eq!(outputs[0], expected_after_reset);
assert_eq!(outputs[1], expected_after_input1);  // Wrong timing!
```

### ✅ **BEST**: Use `.synchronous_sample()` for Clean Logic
```rust
// RECOMMENDED: Simple, direct synchronous logic testing
let outputs: Vec<_> = uut.run(input)?.synchronous_sample().map(|t| t.value.2).collect();
assert_eq!(outputs[0], expected_after_reset);     // Reset cycle
assert_eq!(outputs[1], expected_after_input1);    // First input cycle
assert_eq!(outputs[2], expected_after_input2);    // Second input cycle
```

### ✅ Alternative: Accounting for Multiple Samples (raw output)
```rust
// Only if you need full timing detail - CORRECT but complex
let outputs: Vec<_> = uut.run(input)?.map(|t| t.value.2).collect();
assert_eq!(outputs[3], expected_after_reset_complete);
assert_eq!(outputs[6], expected_after_input1);  // Proper timing
```

### ❌ Wrong: Hardcoded Timing Magic Numbers
```rust
// This breaks if reset cycles or clock behavior changes
assert_eq!(outputs[7], expected);  // Magic number! What does 7 mean?
```

### ✅ Correct: Understanding the Pattern or Using `.synchronous_sample()`
```rust
// Option 1: RECOMMENDED - Use synchronous_sample() to avoid complexity
let outputs: Vec<_> = uut.run(input)?.synchronous_sample().map(|t| t.value.2).collect();
assert_eq!(outputs[1], expected);  // Clear: index 1 = first input cycle

// Option 2: If using raw output, show understanding
// with_reset(1) = ~3 reset samples
// First input = next ~3 samples  
let outputs: Vec<_> = uut.run(input)?.map(|t| t.value.2).collect();
assert_eq!(outputs[4], expected);  // 3 + 1 = first sample of first input
```

## Example: Complete Test Patterns

### **RECOMMENDED**: Using `.synchronous_sample()` 

```rust
#[test]
fn test_my_circuit_clean() -> miette::Result<()> {
    let uut: MyCircuit<U8> = MyCircuit::default();
    
    let test_data = vec![
        bits(0xAB),  // Load this value
        bits(0x56),  // Then this value  
        bits(0x00),  // Then this value
    ];
    
    let input = test_data.with_reset(1).clock_pos_edge(100);
    let outputs: Vec<_> = uut.run(input)?.synchronous_sample().map(|t| t.value.2).collect();
    
    // Clean 1-sample-per-cycle: each index = one clock cycle
    let expected = vec![
        default_output,              // Reset cycle
        compute_expected(0xAB),      // After first input
        compute_expected(0x56),      // After second input  
        compute_expected(0x00),      // After third input
    ];
    
    assert_eq!(outputs, expected);   // Simple, direct comparison!
    Ok(())
}
```

### Alternative: Raw Output with Full Timing Detail

```rust
#[test]
fn test_my_circuit_detailed_timing() -> miette::Result<()> {
    let uut: MyCircuit<U8> = MyCircuit::default();
    
    let test_data = vec![
        bits(0xAB),  // Load this value
        bits(0x56),  // Then this value  
        bits(0x00),  // Then this value
    ];
    
    let input = test_data.with_reset(1).clock_pos_edge(100);
    let output_stream = uut.run(input)?;
    let outputs: Vec<_> = output_stream.map(|t| t.value.2).collect();
    
    // Timing analysis for with_reset(1):
    // Indices 0-2:  Reset cycle - circuit in reset state
    // Indices 3-5:  Process bits(0xAB) 
    // Indices 6-8:  Process bits(0x56)
    // Indices 9-11: Process bits(0x00)
    
    // Test reset behavior
    assert_eq!(outputs[0], default_output);
    assert_eq!(outputs[1], default_output);  
    assert_eq!(outputs[2], default_output);
    
    // Test first input processing
    let expected_after_ab = compute_expected(0xAB);
    assert_eq!(outputs[3], expected_after_ab);
    assert_eq!(outputs[4], expected_after_ab);  // Should be identical
    assert_eq!(outputs[5], expected_after_ab);
    
    // Test second input processing  
    let expected_after_56 = compute_expected(0x56);
    assert_eq!(outputs[6], expected_after_56);
    assert_eq!(outputs[7], expected_after_56);
    assert_eq!(outputs[8], expected_after_56);
    
    Ok(())
}
```

## Summary

### Key Timing Concepts
- **`with_reset(n)`** creates `n` reset cycles at the start
- **`clock_pos_edge(period)`** generates realistic clock timing with multiple samples per input
- **Raw `.run()` output** produces ~3 samples per input representing different clock phases
- **Use `with_reset(1)`** for most tests unless you need extended reset timing

### **RECOMMENDED** Test Writing Approach
- **Use `.synchronous_sample()`** for clean, simple test assertions
- **One sample per clock cycle** - no complex timing calculations needed
- **Direct correspondence** to synchronous logic behavior
- **Matches modern RHDL test patterns** throughout the codebase

### Alternative Approaches
- **Raw output streams** if you need full timing detail
- **Plan your test assertions** around the 3-sample-per-input pattern
- **Debug timing issues** by printing actual output patterns when tests fail

### Quick Reference
```rust
// RECOMMENDED: Clean and simple
let outputs: Vec<_> = uut.run(input)?.synchronous_sample().map(|t| t.value.2).collect();
assert_eq!(outputs[0], reset_val);     // Reset cycle
assert_eq!(outputs[1], after_input1);  // After first input

// Alternative: Full timing detail (more complex)
let outputs: Vec<_> = uut.run(input)?.map(|t| t.value.2).collect();
assert_eq!(outputs[3], after_input1);  // 3 samples into first input cycle
```

This timing behavior is **intentional and realistic** - it simulates how real synchronous digital circuits operate with proper clock phases and timing relationships.

## Implementation Source

The exact behavior described in this guide is implemented in:
- **Clock timing**: `/rhdl/src/rhdl_core/sim/clock_pos_edge.rs` - `ClockPosEdge` state machine
- **Reset logic**: `/rhdl/src/rhdl_core/sim/reset.rs` - `ResetWrapper` implementation

The "3 samples per input" behavior is not an approximation - it's the exact implementation of the 5-state state machine that generates realistic clock waveforms for digital circuit simulation.