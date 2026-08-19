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
| 2 | Both mixers claim a ready they do not honour, and lose the sample silently | **correctness trap** |
| 3 | `MODULATION_CONTROL` is the one latency constant never measured | validation gap |
| 4 | No running regression test for any spectral claim | validation gap |
| 5 | `nco/mod.rs` leads with a superseded recommendation | doc drift |
| 6 | Four smaller items | minor |

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

## 2 — Both mixers claim a ready they do not honour, and lose the sample silently

**Severity:** correctness trap.  Two symptoms of one defect.

`rcstream/bus.rs:66-69` states the convention:

> When used as widget **O** (`SynchronousIO::O`):
>   - `data` is the widget's data flowing *out* to downstream.
>   - `ready` is the widget's ready flowing *out* to upstream
>     (= "am I ready to accept next from upstream?").

Both mixers answer that question with downstream's ready:

- `dsp/mixer/complex_real.rs:208` — `ready: i.downstream_ready`
- `dsp/mixer/complex.rs:184` — `ready: i.downstream_ready`

**The answer is false.**  Each mixer's `out` is a DFF that is
overwritten every cycle whether or not downstream is ready — the
kernels contain no stall path and cannot contain one, because both
input streams are isochronous.  So the mixer *is* always ready to
accept from upstream, and saying "ready only if my downstream is
ready" misdescribes it.

The two symptoms:

1. **The claim is wrong.**  An upstream that believed it would hold its
   sample while the mixer was not ready.  Nothing does today — the
   mixers' inputs are bare `Option<Item<T, ()>>` with no ready path —
   but the field is what a future `RCStreamRelay` insertion would read.
2. **The consequence is unreported.**  Because the mixer consumes
   unconditionally, a cycle with `downstream_ready` low loses the
   registered sample outright.  `Nco` takes the same input and exposes
   `overrun: !i.downstream_ready` (`nco/composite.rs:201`), documented:

   > A sample was presented while `downstream_ready` was low, and is
   > gone. [...] Surfaced because a silently dropped sample is exactly
   > the failure this codebase has shipped before.

   The mixers surface nothing.  On the transmit chain a lost sample is
   reported at the oscillator and silent one stage later at the
   modulator.  `dsp/mixer/mod.rs` and `complex_real.rs` discuss
   starvation — one *input* missing — at length and say nothing about
   downstream not being ready.

### The criterion, since it is easy to get backwards

The right value for an outgoing `ready` is not decided by which
direction the available signal came from.  It is decided by whether the
widget consumes unconditionally:

| widget shape | correct outgoing `ready` | in the tree |
|---|---|---|
| source, no upstream | `true`, vacuously | `nco/composite.rs:198`, `rcstream/util/constant.rs:86` |
| combinational rewire, no register | forward the consumer's ready | `rcstream/util/split.rs:140`, `combine.rs:143` |
| elastic stage with a buffer | that buffer's own ready | `rcstream/map.rs:136`, `relay.rs:137` |
| **non-stalling registered stage** | **`true`, plus an overrun report** | **the two mixers** |

`IqSplit` ANDs both consumers' readies and forwards them, with a doc
comment naming it as the ready toward the upstream source; `IqCombine`
is the mirror image.  Both are **correct** and neither needs changing:
they hold no register, so their ready toward upstream genuinely *is*
their consumer's ready.  An earlier draft of this report listed
`combine.rs:143` as carrying the mixers' defect.  It does not — the
superficial similarity is `i.downstream_ready` appearing in both, and
the difference that matters is the DFF.

**Fix:** `ready: true` in both mixers, carrying the `Nco` comment's
reasoning, and `overrun: !i.downstream_ready` added to both `Out`
structs with the reset suppression `Nco` uses at `composite.rs:205`.
Plus a Tier-2 test on the model of
`composite::tests::a_lost_sample_is_reported`.  The two changes are one
fix: `ready: true` states that the widget always consumes, and `overrun`
reports the consequence of having done so.

---

## 3 — `MODULATION_CONTROL` is the one latency constant never measured

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

## 4 — No running regression test for any spectral claim

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

## 5 — `nco/mod.rs` leads with a superseded recommendation

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

## 6 — Minor items

**6a — `complex_real.rs:47` says "idle" for a cycle that emits a
sample.** The docs read:

> So a mismatch sets `starved` and the output is idle for that cycle.

The kernel emits `Some(Item { data: zero })`, not `None`.  Emitting a
defined zero is the right choice for an isochronous stream — and the
inline comment in the kernel says so clearly — but "idle" has a specific
meaning in `rcstream/bus.rs:24` (`None` = idle, TVALID = 0) and this is
not it.

**6b — `rounding::convergent` has two undocumented preconditions.**
`dsp/mixer/rounding.rs:28` computes `v + half` at `PROD_W`, relying on
the product not using its full width.  It holds at both instantiated
configurations (`ComplexRealMixer` at `PROD_W = A_W + B_W` has one spare
bit; `ComplexMixer` at `A_W + B_W + 1` has more) and the module docs
explain why the *product* cannot overflow — but not why the product
*plus half an LSB* cannot.  Separately, `1 << (DROP - 1)` at line 24
underflows for `DROP = 0`, i.e. a no-narrowing instantiation, which no
`const _: () = assert!` rules out.  Neither is reachable today; both are
one width change away.

**6c — `sin_cos_linear_interp_kernel` has no reset block.**
`sin_cos_linear_interp.rs:295` takes `_cr: ClockReset` and has no
`if cr.reset.any()` block, against CLAUDE.md §12 rule 12.  Defensible —
the widget's state is entirely in the `delayed` DFF and the two BRAMs,
which reset themselves, and the kernel is otherwise combinational.  It
is the only kernel in `dsp` that does this and it carries no comment
saying why, so the next reader has to re-derive that it is fine.  A
one-line comment closes it.

**6d — Five clippy warnings in `dsp` test code.**
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

1. Finding 2 — do this first, not second.  It changes both mixers' `Out`,
   which invalidates their Tier-3 snapshots and Tier-5 digests, so doing
   it before finding 1 means `complex_mixer`'s artifacts are generated
   once against final behaviour rather than twice.
2. Finding 1 — mechanical, closes the contract violation.
3. Finding 5 — documentation only.
4. Finding 4 — one non-ignored spectral regression test.
5. Finding 6 — fold into whichever commit touches each file.
6. Finding 3 — needs the `Nco`-scope decision first, so it should be
   raised with the user rather than chosen unilaterally.
