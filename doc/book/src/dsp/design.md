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
group delay ........... 6862 input samples = 54.9 us
  cascade 2427, int pipe 33, comb pipe 1960, out reg 2, comp 2440
  largest is the compensator at 2440; loop bandwidth ~ 1.8 kHz
alternative ........... [4, 61, 2] cost 96.8 (feasible, but costlier by the rate-weighted model)
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

Every ordered way of factoring the decimation into at most
`max_chain_stages` stages is designed, and the cheapest feasible one
wins. Order matters, because each stage sees a different fraction of
its own output band -- `8 x 61` and `61 x 8` are different filters. The
runner-up is reported, because "a cascade would have been better here"
is exactly the sort of conclusion that should be visible rather than
implied.

Three stages is the default ceiling rather than two, because a deep
decimation often does split better three ways: at `/1024` with a 16 kHz
band, `[128, 8]` costs 65.0 and `[4, 32, 8]` costs 49.0. Each extra
stage multiplies the search, so the ceiling is a budget rather than an
aspiration.

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
width dominates far more than the depth does: 50 dB across a wide
transition (`stopband_edge: 0.9`) costs 17 taps, and the same 50 dB
across a narrow one (`0.6`) costs 53.

The requirement is on the **composite** — the cascade and the
compensator together, which is what decides whether out-of-band content
reaches the output. That is worth stating because the obvious reading is
wrong in a specific way: since the cascade contributes its own rolloff
above the stopband edge, you might expect to be able to buy attenuation
with CIC stages, which are cheap, instead of taps, which are not. You
cannot. A deeper CIC droops harder, the compensator must boost harder to
undo it, and that boost spills past the passband edge — the two effects
very nearly cancel. Extra CIC depth does not buy stopband; it merely
stops *costing* stopband, which it did while the figure was measured
against the compensator alone.

## Lowering it to hardware

Everything above computes the parameters. `cic_chain!` goes the rest of
the way:

```rust,ignore
{{#rustdoc_include ../code/src/dsp/design.rs:macro}}
```

That expands to a module containing a pruned CIC per stage, cascaded
through their framing — `[8, 61]` here, so two framed stages — plus the
derived taps and the compensating FIR. Build the decimation chain with
`narrowband_chain::new()`.

### The compensator is emitted beside the chain, not inside it

`Chain` is the decimation alone. A compensator does not have to sit
immediately behind the decimator, and does not have to be in the FPGA
at all — you may apply the taps further down the fabric, or on a host
after capture. So the macro gives you the pieces and lets you place
them:

| | what it is |
|---|---|
| `Chain` / `new()` | decimation only |
| `Fir` / `compensator()` | the filter, unplaced |
| `Compensated` / `compensated()` | the opt-in, compensator right behind the decimator |
| `TAPS`, `TAP_SHIFT` | the coefficients as plain integers, for a compensator this type cannot reach |

Because skipping the compensator is a real choice, the cost of skipping
it is reported too:

```rust,ignore
narrowband_chain::DROOP_DB   // -19.586  — the chain unaided
narrowband_chain::RIPPLE_DB  //   0.0689 — if the taps are applied
```

Nineteen decibels of droop across the band is not a rounding error, so
for that spec the compensation matters a great deal — but *where* it
happens is a system decision, and the macro does not make it for you.

**The design runs during compilation.** Not in a `const fn`: choosing a
split needs a least-squares fit per candidate tap count, which needs
floating point, which `const fn` does not have on stable. So it happens
in the compiler's own process at macro-expansion time, and the results
are substituted as literals — which is exactly what a const-generic
widget parameter needs.

### It emits its working, not just its answer

Every derived number is a `pub const`:

```rust,ignore
narrowband_chain::SPLIT              // [8, 61]
narrowband_chain::TAPS               // the eleven coefficients
narrowband_chain::TAP_SHIFT          // their fractional bits
narrowband_chain::REGISTER_BITS      // 269
narrowband_chain::RIPPLE_DB          // 0.0689
narrowband_chain::ALIAS_REJECTION_DB // 67.3
narrowband_chain::SNR_DB             // 84.6
```

and the design report becomes rustdoc on the generated module.

This is deliberate. A macro that silently picked five stages and a
51-bit accumulator would be doing something a hardware engineer needs
to audit. The convenience is in not having to *compute* the numbers,
not in not being allowed to see them.

### It fails at compile time, with the reason

An infeasible specification is a compile error naming the requirement,
the shortfall and the knob:

```text
error: cic_chain! cannot satisfy this specification.

       rejection and flatness are jointly infeasible here: every depth
       that rejects well enough droops more than the compensator can
       invert. Best ripple 138.8363 dB against the 0.1000 dB asked for.
       The knob is `alias_free_bw`: a band further from the first null
       both rejects better and droops less.
```

(That is the message for `alias_free_bw = 120e3, alias_db = 90` at
`/488` — asking for 90 dB of rejection across almost the whole output
band. The 138 dB of residual ripple is not a typo: the depth needed for
that rejection droops so steeply that no tap count within the budget
comes close to inverting it.)

A macro that said only "could not design" would turn a solvable problem
into a mystery.
