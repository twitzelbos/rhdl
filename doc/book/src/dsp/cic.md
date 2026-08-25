# CIC Decimator

A cascaded integrator-comb filter is the standard way to decimate hard
without multipliers: `N` integrators at the input rate, decimate by
`R`, then `N` combs with differential delay `M`.

## Why it is the right filter in front of a decimator

Its magnitude response is

```text
           |  sin(π f R M)  | N
|H(f)|  =  | -------------- |
           |    sin(π f)    |
```

which is **zero at every `f = k/(RM)`** — exactly the frequencies that
decimating by `R` folds onto DC. A CIC puts its nulls precisely where
the aliases come from, and needs no coefficients to do it.

## Why it needs a compensator

The same expression droops across the passband. That is not a separate
defect to be fixed; it is the *same* `sinc^N` shape seen from the other
side. More stages mean deeper nulls **and** steeper droop:

| `N`, `R = 32`, passband 0.8 | droop at the edge |
|---|---|
| 2 | 4.8 dB |
| 3 | 7.2 dB |
| 4 | 9.7 dB |
| 5 | 12.1 dB |

See [Compensation](compensation.md).

## Two's-complement wrap is load-bearing

The integrators overflow, deliberately. Hogenauer's bound
`w_in + N·log2(R·M)` is the width at which those wraps *cancel* in the
combs; below it the output is not noisy, it is wrong — and wrong in a
way that looks like a plausible signal. `Default` asserts the width
rather than documenting it.

```admonish warning title="This failure hides until it matters"
A too-narrow accumulator is invisible unless the signal drives the
cascade near its worst case. A sign-alternating stimulus never
integrates that far, so a test built on one reports everything is
fine. The failure waits until someone feeds the filter a large DC
offset — which is why the width is checked at construction and the
gold-model tests drive the stimulus to full scale.
```

## Idle cycles hold the filter

`sample: None` advances nothing — not the integrators, not the
decimation phase. A CIC's state is a running sum over *samples*, not
over cycles, so a gap in the stream must not be read as a zero. That is
what makes the widget correct on a gated stream, and what lets a
compensating FIR sit behind it and see one sample in `R`.

## The marker defines the decimation grid

A marked sample becomes sample zero of a fresh window: the filter
clears its state and restarts its phase, so the next output falls
exactly `R` samples later and contains nothing from before the trigger.

Clearing the state is not optional. An `N`-stage cascade's effective
window is `N·R·M` samples, so realigning the phase alone would still
leak pre-trigger history through the integrators. Realigning without
clearing is the subtly-wrong version of this feature.
