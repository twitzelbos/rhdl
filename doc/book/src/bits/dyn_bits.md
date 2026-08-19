# Dynamic Width Bits

In general to work with dynamic width bitvectors, you will do the following:

1. Start with statically sized bitvectors `Bits::<N>`.
2. Convert them to dynamic bitvectors using `.dyn_bits()` or using the extension operators, like `xadd`, `xmul`, etc.
3. When the calculations are complete, convert the `DynBits` back to a compile-time sized `Bits::<N>` by casting it with `.as_bits()`.  This call is generic over the number of bits in the output, but will panic if there is a mismatch between the runtime size, and the compile time size of the destination.

An example is simpler to understand.  

```rust
{{#rustdoc_include ../code/src/bits/mod.rs:dyn-bits-ex}}
```

While this example is not particularly interesting, it becomes much more difficult when the intermediate operations include bit shifting, sign conversion, etc.  For a more realistic example, consider linear interpolation.  We have two values `a: b8`, and `b: b8`, and an interpolant `x: b4`.  We want to compute as accurately as possible, the expression:

```rust
 c = (a * x + b * (16 - x)) >> 4 
```

This quickly becomes complicated, as the intermediate expressions need to have enough bits to store the products of `b8 x b4 -> b12`, but then a subtraction of a 5 bit literal and a 4 bit unsigned value, needs 6 bits for storage, etc.  However, the end result is still an 8 bit value, and so our function implementation looks like this:

```rust
{{#rustdoc_include ../code/src/bits/mod.rs:lerp-ex}}
```

we can use dynamic bit widths internally to get fine grained control over which bits we keep and which we throw away.  And while in this case, you could hard code all of the intermediate bit widths, it becomes much more convenient when `lerp` is generic over the input and output bit widths.

Note that simply promoting an unsigned `Bits<N>` to a `SignedDynBits` will require an extra bit.  This is because the range of an unsigned `N` bit value is `0..2^N-1`.  To store a positive value of size `2^N-1` requires `N+1` bits in a signed 2's complement integer.  Hence the need for an extra bit.

The process for `SignedBits` is entirely analogous.  A `SignedBits<N>` typed value can be converted to a `SignedDynBits` value using the `.dyn_bits()` method.  This bit vector will have it's size erased from the type signature, and will track the bit width at runtime.  You can then operate on the `SignedDynBits` using either the bit-width preserving operators (like `+`, `-`), which will wrap out of range results, or using the extended operators (like `xadd, xmul, xext`) which will preserve bits, but require more/different output bit widths.  When ready, you can then convert the `SignedDynBits` value back to a `SignedBits` of the correct width.

```admonish note
The goal of `DynBits` is not to have RHDL do a bunch of magic extra bit manipulation for you.  Instead it is to enable you to have precise control over how the bits are manipulated and where they are dropped or preserved.
```

## `xmul` and DSP inference

`xmul` matters for more than bit bookkeeping: **the operands it emits decide
what a vendor synthesiser can map the multiply onto.**

A Xilinx DSP48E1 multiplier is `18 x 25`. So an `18 x 14` product fits one
slice, while a `32 x 32` multiply needs several. RHDL emits `xmul` with its
operands at their *declared* widths and lets the destination width size the
operation, which is how Verilog's `*` already behaves — the operands are
extended, per their own signedness, to the width of the assignment target:

```verilog
reg signed [17:0] a;
reg signed [13:0] b;
reg signed [31:0] p;
p = $signed(a) * $signed(b);      // an 18x14 multiply
```

This used to widen both operands to the result width before multiplying, so
the same expression emitted as `32 x 32` and one-slice-versus-several rested
on the synthesiser recovering the operand widths by bit-range analysis.

The practical consequence for your kernels: **form each product at its
natural width instead of resizing operands up front.** These compute the
same value, and the second is the one that maps cleanly:

```rust,ignore
// Emits a wide multiply -- the operands are resized before multiplying.
let p = (a.resize::<48>() * b.resize::<48>()) >> shift;

// Emits a narrow multiply -- the product width comes from `xmul`.
let p = a.dyn_bits().xmul(b.dyn_bits());
```

```admonish note
RHDL does not instantiate DSP48E1 primitives — it emits behavioural
Verilog and relies on vendor inference. Emitting narrow operands is
therefore the whole of the lever available: it does not force a mapping,
it stops obscuring the one you want. Chaining a second `xmul` by a
*constant* is free in slice terms, since a constant operand lowers to
shift-adds.
```
