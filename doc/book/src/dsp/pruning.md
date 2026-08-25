# Hogenauer Pruning

A plain CIC runs every stage at the worst-case accumulator width. That
is correct and wasteful: at `W_in = 18, N = 5, R = 1024` it is 68 bits
in each of ten registers, plus ten adders to match.

Hogenauer's §V analysis says later stages may discard low-order bits,
because **less of the remaining filter is left to amplify their
truncation noise**. The result is a taper:

```text
  42        39        34        29        27        26        25        24
+----+    +----+    +----+    +----+    +----+    +----+    +----+    +----+
| I1 |>>3 | I2 |>>5 | I3 |>>5 | I4 |>>2 | C1 |>>1 | C2 |>>1 | C3 |>>1 | C4 |
+----+    +----+    +----+    +----+    +----+    +----+    +----+    +----+
```

At `N = 5, R = 1024` that is 517 register bits instead of 680.

## The pruning costs nothing in logic

Each `>>k` is a constant shift feeding a narrowing assignment, which
folds into a bit select — `r28 = r76[12:1]` in the emitted Verilog. The
saving is register bits and adder width with no shifter added anywhere.

## Why it is a macro

The schedule gives a **different width per stage**, which a homogeneous
`[SignedBits<W>; N]` cannot hold and const generics cannot compute
without `generic_const_exprs`. So `cic_pruned!` substitutes literals
into `prune::stage_width`, a `const fn`. The widths are not *asserted*
against the analysis — they *are* the analysis, by substitution, so
they cannot drift.

The whole approach rests on one observation: Hogenauer writes
`B_j = floor(B_out − ½·log2(2·N·S_j))`, and since `S_j = Σ h_j(k)²` is
an integer, that is exactly `B_out − ceil_log4(2·N·S_j)` — computable in
integer arithmetic, hence in a `const fn`, hence usable in a type
position.

## A pruned register does not hold the value

It holds the value divided by `2^(full − W_j)`. Two consequences:

- The **output's LSB weighs more**. Swapping a plain decimator for a
  pruned one of the same `(N, R, M)` changes the output's scale, not
  just its width.
- The **input must be rescaled into stage one**, not merely
  sign-extended. When stage one happens to be unpruned the two widths
  are equal and the rescaling is a no-op — which is exactly how a
  missing rescale survived its first behavioural test, because that
  schedule did not prune stage one. Sweep the configuration.

## What the schedule does not promise

That the resulting noise is acceptable for *your* signal. `b_out` is a
budget; the schedule spends it evenly, making every stage contribute
roughly equal error. Whether the total is small enough is a question
about your measurement — which is what
[Specifying a chain](design.md) derives it from.
