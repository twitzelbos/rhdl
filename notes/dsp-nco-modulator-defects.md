# `dsp::nco` and `dsp::mixer` audit — defect report (2026-08-19)

## Context

Read-only audit of the NCO (`crates/rhdl-fpga/src/dsp/nco/`, 10
modules) and the modulator (`crates/rhdl-fpga/src/dsp/mixer/`, plus
`dsp/iq.rs`), covering source, tests, rustdoc, examples, committed
artifacts, and CHANGELOG entries.  No behaviour was changed; this file
is the record so the findings do not have to be rediscovered.

**Baseline at time of audit** — commit `45808cb0`, everything green:

```
cargo test -p rhdl-fpga --lib dsp::
  110 passed; 0 failed; 11 ignored
```

The 11 ignored are 8 long-running `model::sweep_report::*` and 3
`sin_cos_linear_interp::tests::scratch_*` diagnostics.  `iverilog` is
installed on the audit machine, so all nine widgets' Tier-4 RTL **and**
NTL round-trips genuinely ran.

**Nothing below is a defect in the arithmetic.**  The accumulator's
offset-independence, the phase-truncation direction, the Taylor
rotation, the two's-complement conventions across the composers, and
the convergent narrowing all check out and are well tested.  The
findings are a contract violation, two correctness traps in the newest
code, two validation gaps, and some documentation drift.

## Severity summary

| # | Finding | Severity |
|---|---|---|
| 1 | `ComplexMixer` has no example and no committed waveform trace | **contract violation** |
| 2 | Both mixers put downstream's ready into the outgoing `stream.ready` | **correctness trap** |
| 3 | Neither mixer reports an overrun, though both take `downstream_ready` | **correctness trap** |
| 4 | `MODULATION_CONTROL` is the one latency constant never measured | validation gap |
| 5 | No running regression test for any spectral claim | validation gap |
| 6 | `nco/mod.rs` leads with a superseded recommendation | doc drift |
| 7 | Four smaller items | minor |

---

## 1 — `ComplexMixer` has no example and no committed waveform trace

**Severity:** contract violation — CLAUDE.md §12 rule 8, §6 Layer C.

`crates/rhdl-fpga/src/dsp/mixer/complex.rs` carries the schematic
symbol and all five test tiers, including both `iverilog` round-trips
and a VCD digest.  It has **no** `#Example` section, no
`crates/rhdl-fpga/examples/complex_mixer.rs`, and no
`crates/rhdl-fpga/doc/complex_mixer.md` — while
`crates/rhdl-fpga/vcd/complex_mixer/` does exist, so the widget is
half-way through the artifact contract.

Every other widget in `dsp` has both.  Audited by grepping for the two
`include_str!` forms:

| file | example include | trace include |
|---|---|---|
| `nco/phase_accumulator.rs` | yes | yes |
| `nco/phase_composer.rs` | yes | yes |
| `nco/frequency_composer.rs` | yes | yes |
| `nco/modulation.rs` | yes | yes |
| `nco/ramp.rs` | yes | yes |
| `nco/sin_cos_linear_interp.rs` | yes | yes |
| `nco/composite.rs` | yes | yes |
| `mixer/complex_real.rs` | yes | yes |
| **`mixer/complex.rs`** | **no** | **no** |

The 2026-08-19 CHANGELOG entry's **Paths** line lists only
`examples/complex_real_mixer.rs` and `doc/complex_real_mixer.md`, so
this reads as an oversight rather than a decision.  Under §15 the
`dsp::mixer` work is therefore *In progress*, not *Done*.

**Fix:** add `examples/complex_mixer.rs` on the model of
`examples/complex_real_mixer.rs`, run it to generate
`doc/complex_mixer.md`, commit both, and add the two `include_str!`
lines to the module rustdoc.

---

## 2 — Both mixers put downstream's ready into the outgoing `stream.ready`

**Severity:** correctness trap.  Not an active functional bug; actively
misleading, and spreading.

`rcstream/bus.rs:66-69` states the convention:

> When used as widget **O** (`SynchronousIO::O`):
>   - `data` is the widget's data flowing *out* to downstream.
>   - `ready` is the widget's ready flowing *out* to upstream
>     (= "am I ready to accept next from upstream?").

Both mixers put the opposite direction's signal in that field:

- `dsp/mixer/complex_real.rs:208` — `ready: i.downstream_ready`
- `dsp/mixer/complex.rs:184` — `ready: i.downstream_ready`

Compare the widgets that follow the convention:

- `rcstream/map.rs:136` — `ready: q.input.ready` (the skid buffer's own ready)
- `rcstream/relay.rs:137` — `o.ready = !q.inner.stop_out`
- `rcstream/util/constant.rs:86` — `ready: true`, with docs explaining
  it has no upstream to backpressure
- `dsp/nco/composite.rs:198` — `ready: true`, with the comment *"`stream.ready`
  is vacuously `true` — the NCO has no upstream to backpressure.  It is
  present because the type carries both directions, not because it means
  anything here."*

`Nco` gets this right and says why.  The mixers, written two days later,
do not.

**Why it is not breaking today:** the mixers' inputs are bare
`Option<Item<T, ()>>`, not `RCStream`, so there is no ready wire back to
either upstream and nothing consumes the field.  **Why it still
matters:** the field advertises a combinational ready pass-through that
does not exist.  A future composition that wires this output's `ready`
back to an `RCStreamRelay` upstream of the mixer would be feeding the
relay the ready of a stage *past* the mixer — correct only by the
accident that the mixer never stalls, and silently wrong the moment that
stops being true.

**It is propagating.**  `rcstream/util/combine.rs:143`, from the
2026-08-19 `rcstream::util` commit, copies the same pattern.

**Fix:** `ready: true` in both mixers and in `combine`, with the `Nco`
comment's reasoning.  These widgets are isochronous and never
backpressure an upstream, so vacuously-ready is the honest value.
Then `downstream_ready` is left doing nothing, which leads to finding 3.

---

## 3 — Neither mixer reports an overrun, though both take `downstream_ready`

**Severity:** correctness trap — a dropped sample is silent at the
modulator and reported at the NCO.

`Nco` takes `downstream_ready` and exposes `overrun: !i.downstream_ready`
(`nco/composite.rs:201`), documented at length:

> A sample was presented while `downstream_ready` was low, and is gone.
> [...] Surfaced because a silently dropped sample is exactly the
> failure this codebase has shipped before.

Both mixers take the same input, are equally isochronous, equally
unable to stall — and do nothing with it except echo it into the field
from finding 2.  `dsp/mixer/mod.rs` and `complex_real.rs` discuss
starvation (one input missing) at length and say nothing at all about
downstream not being ready.

So on the transmit chain, a lost sample is reported at the oscillator
and silent one stage later at the modulator.  That asymmetry is not
argued for anywhere.

**Fix:** add `overrun: bool` to both mixers' `Out`, mirroring `Nco`
including the reset suppression at `composite.rs:205`, and a Tier-2
test on the model of `composite::tests::a_lost_sample_is_reported`.
This subsumes finding 2's leftover: `downstream_ready` then has a real
consumer.

---

## 4 — `MODULATION_CONTROL` is the one latency constant never measured

**Severity:** validation gap, against the module's own stated standard.

`nco/latency.rs` opens with:

> A latency constant that has never been checked against the hardware is
> a comment that the scheduler trusts with the experiment's phase
> coherence.  Every constant below has a test in this module that
> measures the real latency in simulation and fails if they disagree.

That holds for `PHASE_COMPOSER`, `FREQUENCY_COMPOSER`,
`ACCUMULATOR_PHASE_OFFSET`, `ACCUMULATOR_FREQUENCY_WORD`,
`PHASE_TO_AMPLITUDE`, and `MODULATION_INPUT` — each has a
`*_latency_is_as_declared` test.  `composite.rs`'s
`end_to_end_latency_matches_the_constants` then measures `PHASE_CONTROL`
and `FREQUENCY_CONTROL` through the assembled `Nco`, which is the first
place they can be checked as a chain.

`MODULATION_CONTROL` (`latency.rs:106`) is checked only as arithmetic —
`composed_totals_are_what_the_scheduler_expects` asserts it equals 4,
which restates its own definition.

The reason it cannot be measured end-to-end is structural:
`ModulationInput` and `FrequencyRamp` are **not** inside the `Nco`
composite.  A caller wires `ModulationInput`'s `.word` into
`frequency_composer::In::modulation` and `FrequencyRamp`'s `.word` into
`master` or `scheduled_offset` externally, and no test performs that
wiring.  So the composite's `In` exposes `modulation` as a raw
`Bits<48>` term and the one constant the scheduler needs for
eddy-current compensation is the one nothing measures.

This is the same class of error `latency.rs` already documents having
made once — reading `sin_cos`'s test shift of 2 as a hardware latency
and putting a wrong constant into the scheduler's arithmetic.

**Fix (either is defensible, and the choice should be explicit):**

- A test that wires `ModulationInput` → `Nco.frequency.modulation` and
  `FrequencyRamp` → `Nco.frequency.master` in a harness, and measures
  modulation-sample-to-`(sin, cos)` directly; or
- a composite that contains them, if the intended deployment shape is
  one widget rather than a scheduler-assembled chain.

The second is the larger question and should not be decided inside a
test.  It is worth asking whether `Nco` is meant to be the deployment
unit or a subassembly.

---

## 5 — No running regression test for any spectral claim

**Severity:** validation gap.

The spectral numbers are load-bearing in the docs and in the design
decisions they justify:

| claim | where | test that produces it |
|---|---|---|
| −116.1 dBc, linear interp at 10/12 | `nco/mod.rs`, `sin_cos_linear_interp.rs` | `model::sweep_report::linear_vs_cordic` — `#[ignore]` |
| linear beats 8-stage CORDIC by 26 dB | `nco/mod.rs` | same, `#[ignore]` |
| convergent −103.0 dBc vs round-half-up −98.0 | `mixer/mod.rs` | not in the tree at all (design-note measurement) |
| wrapping costs up to 96 dB, per-word table | `sin_cos_linear_interp.rs` | `scratch_overflow_spectrum` — `#[ignore = "diagnostic"]` |
| 14-bit DAC costs ~3 dB | `nco/config.rs` | `sweep_report::arithmetic_precision_cost` — `#[ignore]` |

The chain is not broken — `model::tests::measured_sfdr_tracks_the_truncation_formula`
runs and validates the model against `6.02·P − 3.92`, and
`sin_cos_linear_interp::tests::model_agrees_with_the_widget` ties the
bit-exact model to the widget's actual output sample-for-sample.  That
pairing is good work and is exactly the substitution CLAUDE.md §TL;DR
forbids skipping.

But no green test would fail if a datapath change cost 20 dB of SFDR.
The spur figures are the reason the architecture was chosen over a
bigger LUT and over CORDIC; a regression in them would surface as a
bench measurement months later.

**Fix:** one non-ignored spectral test on the widget's own output —
a single well-chosen adversarial tuning word, a 65536-point FFT
(the machinery is already in `model::{blackman_harris, fft}` and
already used from `sin_cos_linear_interp.rs:924`), asserting the worst
in-band spur is below a threshold with generous margin.  One word at
~1 s of runtime buys the regression that the full ignored sweep cannot,
because the sweep never runs.

Also worth recording: the convergent-vs-round-half-up measurement that
decided the rounding rule lives only in `../ocra2/docs/modulator_design_note.md`,
outside this repository.  The CHANGELOG cites the numbers; nothing in
the tree reproduces them.

---

## 6 — `nco/mod.rs` leads with a superseded recommendation

**Severity:** documentation drift.  Live-looking advice that the built
widget does not follow.

`nco/mod.rs:134` states, in bold:

> **Recommendation: `P = 13`** — 78 dB worst case, 8 dB of margin over a
> 70 dB target, one BRAM36, sin and cos from the single table.

That is the plain-LUT conclusion.  Roughly 70 lines later the doc
pivots to linear interpolation and records the decision properly
("**Decision: linear interpolation, no parameterised rotator.**").  The
widget as built uses **10 coarse bits** (2 quadrant + `TBL_W = 8`) and a
256-entry table — not P=13 — and reaches −116 dBc rather than 78 dB.

A reader scanning for the sizing answer hits the bold P=13 line first.
The pivot is recorded but not signposted from the superseded
recommendation.

Two smaller items in the same file:

- The module list at `nco/mod.rs:6` names only `phase_accumulator`, and
  the "# Planned structure" section is still framed as forward-looking.
  Eight further modules exist: `composite`, `config`, `frequency_composer`,
  `latency`, `model`, `modulation`, `ramp`, `sin_cos_linear_interp`
  (`nco/mod.rs:206-215`).
- The sizing section is headed "Sizing the phase-to-amplitude stage" and
  opens "Phase-to-amplitude conversion is **not built yet, on purpose**."
  It has been built since 2026-08-17 (`bbba35f7`).

**Fix:** mark the P=13 recommendation as superseded at the point it is
made, with a forward reference to the interpolation decision; update the
"not built yet" framing; list the modules that exist.

---

## 7 — Minor items

**7a — `complex_real.rs:47` says "idle" for a cycle that emits a
sample.** The docs read:

> So a mismatch sets `starved` and the output is idle for that cycle.

The kernel emits `Some(Item { data: zero })`, not `None`.  Emitting a
defined zero is the right choice for an isochronous stream — and the
inline comment in the kernel says so clearly — but "idle" has a specific
meaning in `rcstream/bus.rs:24` (`None` = idle, TVALID = 0) and this is
not it.

**7b — `rounding::convergent` has two undocumented preconditions.**
`dsp/mixer/rounding.rs:28` computes `v + half` at `PROD_W`, relying on
the product not using its full width.  It holds at both instantiated
configurations (`ComplexRealMixer` at `PROD_W = A_W + B_W` has one spare
bit; `ComplexMixer` at `A_W + B_W + 1` has more) and the module docs
explain why the *product* cannot overflow — but not why the product
*plus half an LSB* cannot.  Separately, `1 << (DROP - 1)` at line 24
underflows for `DROP = 0`, i.e. a no-narrowing instantiation, which no
`const _: () = assert!` rules out.  Neither is reachable today; both are
one width change away.

**7c — `sin_cos_linear_interp_kernel` has no reset block.**
`sin_cos_linear_interp.rs:295` takes `_cr: ClockReset` and has no
`if cr.reset.any()` block, against CLAUDE.md §12 rule 12.  Defensible —
the widget's state is entirely in the `delayed` DFF and the two BRAMs,
which reset themselves, and the kernel is otherwise combinational.  It
is the only kernel in `dsp` that does this and it carries no comment
saying why, so the next reader has to re-derive that it is fine.  A
one-line comment closes it.

**7d — Five clippy warnings in `dsp` test code.**
`iter().any()` where `contains()` fits at `frequency_composer.rs:241,245`
and `phase_composer.rs:249,253`; a redundant `u128` cast at
`lerp/fixed.rs:172`.  All in test code.  The crate carries ~340 warnings
overall, so this is a pre-existing baseline rather than a `dsp`
regression — noted only so the `dsp` share is known.

---

## What was checked and found sound

Recorded so a later audit does not repeat the work:

- **Offset independence.** `phase_accumulator` adds `phase_offset` to
  the output and `frequency_word` to the register.  The asymmetry is
  the widget's reason for existing, is correct, and is pinned by
  `removing_an_offset_rejoins_the_untouched_trajectory`.  It is also the
  source of the `FREQUENCY_LEADS_PHASE_BY = 1` skew, correctly derived
  and const-asserted against underflow.
- **Truncation direction.** `composite.rs` takes the top 22 of 48 bits.
  `truncation_takes_the_high_bits` is verified-able-to-fail, and
  `PHASE_TRUNCATION_BITS == 26` is const-asserted against the kernel's
  literal `26`.
- **Table headroom.** `TABLE_SCALE = (1 << 17) - 2`, one LSB below the
  18-bit signed maximum.  `interpolated_sum_never_leaves_the_range`
  evaluates all 2²² phases; combined with `model_agrees_with_the_widget`
  this is a stronger guarantee than a clamp, as the docs claim.
- **The `Option`-payload sign-extension workaround.**
  `modulation.rs`'s explicit sign-fill (`sign_fill = 2^48 − 2^32`)
  is arithmetically correct, and the underlying codegen defect is
  correctly filed as compiler work per §11.1 rather than patched
  in place.
- **Convergent rounding.** `rounding::convergent` is genuine
  round-half-to-even, including for negative operands: the low-`DROP`-bit
  tie test is correct in two's complement, and the odd-result decrement
  steers ties to even.
- **Unit conversions.** `config::{tuning_word, frequency_microhertz,
  phase_word}` round-trip within one resolution step, and
  `resolution_microhertz() * 100 < NARROWEST_LINEWIDTH_UHZ` is
  const-asserted, so a clock change breaks the build rather than the
  physics.
- **Fractional ramp accumulator.** `ramp`'s 16 fractional bits are the
  whole design; `a_ramp_slower_than_one_lsb_per_sample_still_moves` is
  verified-able-to-fail against an integer accumulator, which is the
  failure mode that would silently flatten an adiabatic sweep.
- **Multiplier counts.** `multiplier_count_is_as_claimed` counts `" * "`
  in the emitted Verilog and asserts 2 and 4 — a resource claim made
  checkable, exactly as `mixer/mod.rs` argues it must be.

## Suggested order of work

1. Finding 1 — mechanical, closes the contract violation.
2. Findings 2 and 3 together — one PR per `mixer` widget, or one PR for
   both since it is a single coherent change to the same convention.
   Include `rcstream/util/combine.rs:143` so the pattern stops
   propagating.
3. Finding 6 — documentation only.
4. Finding 5 — one spectral regression test.
5. Finding 4 — needs the `Nco`-scope decision first, so it should be
   raised with the user rather than chosen unilaterally.
6. Finding 7 — fold into whichever PR touches each file.
