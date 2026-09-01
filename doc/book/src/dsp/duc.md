# The Up-Converter

`dsp::duc` assembles the transmit chain: split the complex envelope,
interpolate each arm, recombine, mix onto a carrier.

```text
  env @ f_lo --> [ interpolate x R ] --> @ f_hi --> x carrier --> out
```

Three widgets. `EnvelopeUpsampler` is the shared front end. `IqDuc` adds
the oscillator and a full complex mixer and emits `Iq`, for a quadrature
DAC or an external I/Q modulator — four multiplies. `RealDuc` uses
`RealPartMixer` instead and emits `Real`, for a single DAC — two
multiplies, because `ad + bc` is never formed.

`RealDuc` if one converter carries the signal, which is the usual case
and the cheaper one; `IqDuc` if the passband is formed outside the FPGA.
They are separate widgets rather than one with a flag for the reason
recorded in `dsp::mixer`: `if`/`else` in a kernel lowers to a mux whose
*both* arms evaluate, so a flag would emit four multiplies either way and
the saving would exist only in the documentation.

## What mirrors the down-converter, and what does not

| | down-converter | up-converter |
|---|---|---|
| mixing happens | first | last |
| rate change | decimate, after mixing | interpolate, before mixing |
| `ready` upstream | pass-through | a real once-per-`R` request |
| output cadence | one cycle in `R` | every cycle |
| pruning | Hogenauer §V applies | it does not |
| width tapering | costs noise | costs nothing |

Shared: the same `dsp::nco` oscillator, the same `SyncMark` framing, both
arms of the rate change forced to one type for the same anti-asymmetry
reason, and neither chain normalises the CIC's gain.

**The `ready` row is the one that changes how a chain is assembled.** A
down-converter is *pushed*: samples arrive from a converter and the chain
keeps up or reports an overrun. An up-converter *pulls* — it asks for an
envelope sample once every `R` cycles — so whatever generates the envelope
has to answer that request. A host DMA feeding it needs a FIFO in
between, not a fixed schedule.

## Tapering is free here, and pruning is not available

Both rows in the table above follow from the same asymmetry, and both are
the opposite of the receive-side intuition.

Hogenauer's §V pruning schedule is a *decimator* result: it computes how
much noise each stage may inject given that later stages attenuate it.
Reversing the chain reverses that attenuation, so the schedule does not
transpose. `dsp::cic::interp` carries the argument.

What replaces it is better. An interpolator's stage widths can be trimmed
to each stage's own **growth bound** — the largest value that stage can
produce — and a value that cannot occur cannot be lost. So the taper
injects no error at all: a `cic_interp_tapered!` widget is
**bit-identical** to the uniform `CicInterpolate`, not merely close to it.
`tests/cic_interp_tapered.rs` asserts the equality.

For the worked sizing below, the uniform interpolator spends 270 bits of
state per arm; tapered to the exact bound it is 180 (widths
`17, 18, 19, 18, 24, 30`), and the generated widget spends 181, because it
lifts the non-monotonic fourth stage to the running maximum so that every
inter-stage transfer is a widening. A 33% saving, losslessly.

## A worked sizing

A 16-bit complex envelope at 1 Msps onto a 125 Msps carrier, three CIC
stages, unit differential delay, out to a 14-bit DAC — every parameter
derived rather than guessed:

```text
  W       = 16      envelope width per component
  S       = 3       CIC stages
  M       = 1       differential delay
  R_MAX   = 125     125 Msps / 1 Msps
  WA      = 30      = W + interp::gain_bits(3, 125, 1) = 16 + 14
  CW      = 7       = interp::rate_width(125)
  OW      = 14      the DAC
  PROD_W  = 49      = WA + AMP_W + 1 = 30 + 18 + 1
  DROP    = 35      = PROD_W - OW
```

The gain a caller has to undo is `(R·M)^N / R = 125² = 15625`, which
`interp::dc_gain_ratio` reports as the exact ratio `1953125/125`. Note the
`/R`: an interpolator's DC gain is *not* the decimator's `(R·M)^N`,
because only one input sample in `R` is non-zero.

## Specifying one instead of sizing it

The widths above are consequences. What you actually know is the
requirement:

```rust,ignore
{{#rustdoc_include ../code/src/dsp/duc.rs:spec}}
```

and `interp_chain::design` turns it into the rest:

```rust,ignore
{{#rustdoc_include ../code/src/dsp/duc.rs:derive}}
```

For that spec:

```text
split ............. [5, 25] N=[5, 2] M=[2, 1]
images ............ 66.6 dB down (asked >= 60.0)
ripple ............ 0.0147 dB (asked <= 0.100)
compensator ....... 9 taps at 1.0000 MHz
state ............. 836 bits uniform, 614 as built
group delay ....... 1864 converter samples, largest is the comb pipeline
rates reachable ... 73 of 124
```

Two things in that output are worth stopping on.

## More taps will not improve image rejection

This is the one place where transposing receive-side intuition is simply
wrong, and it is why the report says so in bold.

The compensator runs **before** the rate change, at the envelope rate. Its
response is therefore periodic in the envelope-rate frequency, so the
image at `k + u` sees exactly the gain the signal at `u` sees. Lifting the
signal lifts every image by the same amount, and the image-to-signal ratio
is the cascade's alone.

Contrast the receiver, where the compensator sits *after* the fold and its
stopband is part of the alias budget. There, taps buy rejection. Here they
buy flatness and nothing else, and asking a pre-compensator for a stopband
spends taps on an attenuation that changes no number anyone cares about.

## The knobs that do work on images

In order of what they cost:

- **CIC depth `N`.** The primary knob, and it costs registers rather than
  multipliers. Each stage multiplies the rejection: at a signal occupying
  0.0028 of the envelope Nyquist and `R = 125`, rejection goes
  114 / 171 / 228 / 285 dB for `N` = 2 / 3 / 4 / 5 — roughly 57 dB per
  stage.
- **Signal bandwidth, or equivalently the envelope rate.** The images sit
  *centred* on multiples of the envelope rate, which is exactly where the
  CIC's nulls are, so rejection is set by how far the image band's *edge*
  reaches out of the null. Halving the occupied fraction buys a lot.
- **Splitting the rate**, which moves where the intermediate nulls land
  and — see below — is what makes the next item affordable.
- **A post-compensator**, which is the *only* compensator that can touch
  images.

### The post-compensator, and why it is not the default

[`PostCompensatedInterp`](https://docs.rs/rhdl-fpga) puts the FIR on the
far side of the rate change. At the converter rate the response is no
longer periodic in envelope-rate `u`, so `u` and `k + u` are different
frequencies to it and a stopband requirement becomes a real image
requirement instead of a no-op.

The cost is that the filter's transition band is `(1 − 2·edge)/R` wide, so
**the tap count grows linearly with `R`** — and every tap runs at the
converter clock. From `interp_chain::post_compensator_taps`, at
`passband = 0.4` and 60 dB:

| `R` | 2 | 4 | 8 | 16 | 32 | 125 |
|---|---|---|---|---|---|---|
| taps | 12 | 24 | 48 | 97 | 195 | **755** |

A 755-tap FIR at 125 MHz is not a widget anyone wants. **The useful
reading is not "don't", it is "not here":** put the post-compensator
between *chain stages*, where the local `R` is small. A `5 × 25` split
compensated after its first stage runs a filter at 5 MHz against an `R` of
5 — a couple of dozen taps. That is a second reason to split a transmit
chain, beyond register bits.

So the default is the pre-compensator
([`CompensatedInterp`](https://docs.rs/rhdl-fpga), which is what
`IqDuc` and `RealDuc` reference and what `interp_chain::design` designs):
it is cheap, it flattens the band, and images are the cascade's job. Reach
for the post-compensator deliberately, after checking
`post_compensator_taps`.

## Reaching every rate

`rates reachable ... 73 of 124` is the cost of the split. A two-stage
`5 × 25` chain can only be run at rates that factor across its stages, so
51 of the integer rates from 2 to 125 are unreachable — 29, 31, 37, 41,
43 and so on, every rate with a prime factor the split cannot supply.

Only a single stage divides by every integer, which is what
`arbitrary_rate` asks for:

```rust,ignore
{{#rustdoc_include ../code/src/dsp/duc.rs:arbitrary}}
```

```text
split [125], 351 state bits as built, rate-weighted cost 2.010e10
```

against the split's 614 bits and 9.665e9. **The single stage is smaller
and more expensive**, which is not the trade anyone expects. One deep CIC
at one rate needs fewer registers than two shallower ones with an
inter-stage width, but every one of those registers is clocked at the
converter rate, and the rate-weighted cost model — which is a proxy for
where timing closure gets hard, not an area figure — charges for that.

So the choice is not "cheap versus flexible". It is: pay in *area* for
reachability, or pay in *clocked width at the fast rate* for a smaller
register count. Which one binds depends on the device.

## Gain is not normalised

The output carries the full DC gain. Undoing it costs either a multiply or
a shift that discards bits the filter was built to keep, and which is
right depends on what comes next — so the widget reports the factor rather
than guessing. Same decision as the down-converter, same reason.

## The rate is a run-time input

`R_MAX` sizes the widths at build time; any rate up to it works unchanged.
Two consequences reach a caller:

- **The gain moves with the rate**, and nothing normalises it.
- **A rate change wants a mark with it.** Changing the rate alone leaves
  the output at the old rate's amplitude, because the comb section feeds
  the integrators the `N`-th difference of the envelope and that is zero
  for a steady one. Marking the first sample at the new rate clears the
  cascade so the new gain establishes itself.

## Delay

`group delay ....... 1864 converter samples, largest is the comb pipeline`
is latency for a transmitter and phase margin for a modulator inside a
loop. [Delay and control loops](delay.md) is about the difference, and
about why the largest term here is the pipelining rather than the
compensator.
