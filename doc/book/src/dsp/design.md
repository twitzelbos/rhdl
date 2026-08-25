# Specifying a chain

A CIC decimator and its compensator take a dozen numbers before you can
instantiate one: how many stages, differential delay, accumulator
width, pruning budget, tap count, coefficient width, fractional
bits — per decimation stage. None of those is a requirement. They are
all consequences.

What you actually know is this:

```rust,ignore
{{#rustdoc_include ../code/src/dsp/design.rs:spec}}
```

and `design` turns it into the rest:

```rust,ignore
{{#rustdoc_include ../code/src/dsp/design.rs:derive}}
```

For the spec above that prints:

```text
decimate .............. 488 as [8, 61]  (125.000 MHz -> 256.1475 kHz)
alias-free bandwidth .. 64.000 kHz = 0.500 of output Nyquist
stage 1 ............... /8 N=2 M=1 at 125.000 MHz
  accumulator ......... 22 bits, prune budget 12
  widths .............. [16, 16, 16, 16] = 64 bits
stage 2 ............... /61 N=5 M=2 at 15.625 MHz
  accumulator ......... 51 bits, prune budget 47
  widths .............. [37, 30, 24, 18, 16, 16, 16, 16, 16, 16] = 205 bits
compensator ........... 11 taps
  fractional bits ..... 9, multipliers 6
  taps ................ [-177, 1286, -4829, 12002, -21186, 26320, ...]
register bits ......... 269 (rate-weighted cost 89.6)
achieved ripple ....... 0.0689 dB (asked <= 0.100)
achieved alias reject . 67.3 dB (asked >= 60.0)
achieved SNR .......... 84.6 dB (asked >= 80.0)
alternative ........... [122, 4] cost 97.3 (feasible, but costlier)
```

Everything below the line is derived. Several of those derivations are
worth understanding, because they encode decisions you would otherwise
have to make yourself.

## Why the total decimation is an input

Because the output rate you want is frequently unreachable. From
125 MHz, an exact 256 kHz output needs `R = 488.28125`. `R = 488` gives
256.148 kHz; `R = 512` gives 244.14 kHz. Neither is what you asked for,
and which one you can live with is a system-level decision — a designer
that quietly rounded it would be hiding the most consequential choice
in the chain.

So `R` is given and the achieved output rate is reported.

```admonish tip
122.88 MHz, the usual radio clock, divides by exactly 480 to 256 kHz.
If you are free to choose the converter clock, choosing one that
divides cleanly is worth more than any amount of filter design.
```

## Bandwidth is one-sided

`alias_free_bw_hz` is the edge frequency measured from DC. A complex
stream carrying a channel of total width `B` is entered as `B / 2`, so a
"128 kHz wide" channel is `64e3`.

This matters more than a units convention usually does. Read the other
way, 128 kHz at a 256 kHz output rate is *exactly* the output Nyquist,
which no CIC can deliver — the two readings differ between comfortable
and impossible.

## Flatness and noise are separate budgets

It is tempting to treat "how flat" and "how quiet" as one error budget.
They are not, and conflating them produces a designer that trades them
against each other incoherently.

| Requirement | What it is | Bought with |
|---|---|---|
| `max_ripple_db` | systematic gain error across frequency | **taps** |
| `min_snr_db` | additive broadband noise | **register width** |

Spending taps does nothing for noise; spending width does nothing for
flatness. Tightening one leaves the other's cost untouched, and there
are tests asserting exactly that.

## Cascading is searched, not assumed

A single CIC decimating by 488 needs `16 + N·log2(488)` ≈ 66-bit
accumulators, **all of them clocked at the full 125 MHz**. Split as
`8 × 61`, nothing wider than 16 bits runs at 125 MHz and the wide
registers run at 15.6, where width is cheap and timing is not the
binding constraint.

Both are designed and the cheaper wins. The loser is reported, because
"a cascade would have been better here" is exactly the sort of
conclusion that should be visible rather than implied.

```admonish warning title="The cost model is a proxy"
Cost is register bits weighted by the rate they run at. That is *not*
an area estimate — flip-flops cost the same however slowly they are
clocked, and by plain area the single stage often wins (240 bits
against the cascade's 269). What the weighting captures is where the
difficulty sits: a 66-bit adder at 125 MHz is a different proposition
from the same adder at 15 MHz. Both numbers are reported so you can
judge by whichever binds for you.
```

## It refuses rather than compromising

Ask for something a CIC cannot do and you get told which constraint
failed and how close it came:

```rust,ignore
{{#rustdoc_include ../code/src/dsp/design.rs:infeasible}}
```

Deep rejection across a wide band is the one thing a CIC structurally
cannot give you, because **its nulls and its droop are the same
expression**. More stages reject better *and* droop more steeply, so
chasing rejection with depth makes flatness harder, and no tap count
escapes it. `Unmet::Incompatible` names that tension and points at the
bandwidth, which is the knob that helps.

A synthesiser that quietly returned something off-spec would be worse
than one that refuses: the number you did not get is the number you
were relying on.

## The compensator can do double duty

A CIC's stopband is whatever `sinc^N` happens to give. When that is not
enough — or when something downstream decimates again — the compensator
is the natural place to put the attenuation, because it is already
there and already running at the low rate:

```rust,ignore
{{#rustdoc_include ../code/src/dsp/design.rs:antialias}}
```

Note the `Method::Remez`. A stopband requirement is a statement about
the *worst case*, and least squares minimises the *average* — it will
happily trade a deep notch here for a shallow one there and satisfy the
average while missing the specification. Remez minimises the maximum
weighted error, which is the quantity the requirement is written in.

Attenuation is not free. It is bought with taps, and the transition
width dominates: 50 dB across a wide transition costs 29 taps, and the
same 50 dB across a narrow one costs 67.

## What is not automatic yet

Nothing here writes the widget declaration for you. `ChainDesign` gives
you the numbers; instantiating `cic_pruned!` and `SymmetricFir` with
them is still a manual step, because RHDL widgets are const-generic and
those numbers have to exist at compile time.

Closing that gap is a `cic_chain!` proc macro, which needs the design
math to live somewhere the macro layer is allowed to depend on. Until
then, run the designer, read the report, and paste.
