# Delay and control loops

Every other figure in the chain designer is about the frequency
response: ripple, alias rejection, image rejection, noise. For a filter
that is only *listening* — a receiver, a spectrum display, a logger —
those are the whole story, and delay is free. Put the same filter inside
a feedback loop and the ranking inverts. **Loop bandwidth is set by loop
delay**, and a decimating measurement filter is usually the largest
single contributor to it.

The rule of thumb, and it is only that:

```text
achievable bandwidth  ~  1 / (10 · total loop delay)
```

`cic::delay::loop_bandwidth_hz` is that arithmetic. It is offered as an
aid, not a design rule — the real number depends on the plant and the
controller, and anyone who knows theirs should use the delay figure
directly.

## Delay is a requirement, so it goes in the spec

```rust,ignore
{{#rustdoc_include ../code/src/dsp/delay.rs:spec}}
```

`max_group_delay_s` joins ripple, rejection and SNR as something the
designer must satisfy rather than something you read off afterwards.
Zero means unconstrained, which is the right value for a receiver.

A refusal is the useful answer:

```rust,ignore
{{#rustdoc_include ../code/src/dsp/delay.rs:report}}
```

```text
no split is fast enough: best 60.0 us against 30.0 us asked
```

The refusal carries the shortest delay *any* candidate split achieved,
so the gap between that and the requirement is the size of the problem —
here a factor of two, which is not something a tap or two will close.

## Where the delay goes

Design once with no bound and read the parts. A single total is not
actionable, because **which term is largest depends on the
configuration**:

```rust,ignore
{{#rustdoc_include ../code/src/dsp/delay.rs:budget}}
```

```text
7516 samples total; largest is the comb pipeline at 3750
```

The five terms are:

| term | what it is |
|---|---|
| `cascade` | the filter itself: `N(RM−1)/2`, the centre of mass of `N` cascaded boxcars |
| `int pipe` | the integrator cascade reads registered values, `N−1` at the input rate |
| `comb pipe` | the comb cascade does too, and the combs run at the *output* rate |
| `out reg` | one clock on the stage output |
| `comp` | the compensator, `(taps−1)/2` **of its own samples** |

Two of those are the ones that surprise people, and both for the same
reason: they are counted at the slow rate. The comb section runs after
the rate change, so its `z^-(N-1)` is `(N−1)·R` *input* samples. The
compensator runs there too.

The table below is generated from the maths, not written by hand:

{{#include cic_delay_budget.md}}

Read the first two rows together, then rows three and four. The
`comb pipe` column is the cost of the pipelining that makes both
cascades one adder deep, and in the split configuration it is the
largest single contribution to the loop's delay. Nobody chose it as a
delay decision; it was chosen for fmax.

## The pipelining is not a knob

Both cascades read the previous stage's *registered* output, so the
critical path is one adder however deep the cascade. That is what lets a
five-stage CIC close timing at 125 MHz, and it is not negotiable in a
widget that has to.

Selecting between the two with a `const PIPELINE_COMBS: bool` and an
`if` does not work, for the reason recorded in `dsp::mixer`: `if` lowers
to a mux and **both branches always evaluate**, so the unselected
combinational cascade stays in the netlist and the *default* path
silently loses the fmax the pipelining exists to buy.

So `pipelined_combs` in the spec is a statement about *which
implementation you are pricing*, not a switch on the shipped widget.
`false` is a software CIC, a vendor core, or a hand-written block — and
that is what the next section is about.

## Fabric or the processor

On a Zynq the obvious question is whether the loop needs the fabric at
all: an ARM core running a real-time thread can close a few-kHz PID
comfortably. The delay maths sharpens the question, and then answers a
different one than you expect.

For the lock-in spec above the designer returns a two-stage split, and
the per-stage figures are lopsided:

```text
shapes [(N=1, R=10), (N=4, R=125)]
  stage 0: total 5.5 samples   (comb pipeline 0)
  stage 1: total 6261 samples  (comb pipeline 3750)
```

(Half samples are real: the centre of mass of an even-length boxcar falls
between two samples.)

A decimation chain's stage `k` is referred to the input rate by the
product of the factors *ahead* of it, so the same register costs
hundreds of times more in the tail than in the head. Here the head stage
contributes five and a half samples out of seven and a half thousand.
`decimation_stage_breakdowns` exists to say so: "the comb pipelining
costs 3750 samples" is not actionable, and "*stage 1's* comb pipelining
costs 3750 samples" is.

Which suggests moving the tail into software — and the same chain priced
with `pipelined_combs: false` is 3766 samples instead of 7516 — a factor
of two, which is exactly the factor the 30 µs requirement was short by.

**And that is where the two constraints collide.** The tail stage here
runs at 12.5 Msps, because the head only decimates by ten. No real-time
thread consumes 12.5 Msps sample-by-sample, so the stage whose registers
are expensive is also the stage that cannot leave the fabric. Pushing
more decimation into the head to get the rate down moves delay into the
head — where it is cheap — but the head then needs the comb stages, and
their pipelining is what costs.

The design move the breakdown actually points at is therefore neither
"use the fabric" nor "use the ARM": it is to **reshape which stage
carries the depth**. Depth in the head is nearly free in delay terms;
depth in the tail is not. That is a search the designer can do, and it
is why `max_group_delay_s` is a constraint on the search rather than a
figure printed after it.

## What this does not model

The group delay here is the *filter's*, and a loop has more in it:

- The converters, their own pipelining, and everything else in the
  fabric path.
- For a software leg, the scheduling behaviour of the thread. A PID cares
  about delay *variation* at least as much as mean delay, and a
  real-time thread's worst case is not its average. The fabric's delay,
  by contrast, is exact and constant — which is frequently the reason to
  use it, quite apart from throughput.
- Any transport between the two, which on a Zynq means an AXI crossing.

The figure is a floor on the loop's delay, not an estimate of it.

## The formulas are checked, not derived and hoped

A CIC's composite response is a boxcar of length `R·M` cascaded `N`
times. That is symmetric, so the filter is linear phase and its group
delay is the centre of mass of its impulse response. Both closed forms —
they differ, because an interpolator has one more handover register than
a decimator — are verified against a numerically computed centre of mass
over `N ∈ {2,3,5}`, `R ∈ {4,10,50}`, `M ∈ {1,2}` and both pipelining
choices.

An earlier version was off by exactly one sample, and an earlier version
than that applied the decimator's formula to an interpolator and was
wrong by a factor of 125 — silently, because the two expressions have
the same shape. Neither survived the impulse-response check, which is
the reason it exists.
