# Package Manager and IP Registry Architecture for RHDL — Design Plan

> **Status: design plan, not committed engineering work.** This document defines the architecture for first-class hardware IP distribution via cargo and a curated registry overlay. It is the highest-leverage feature on the roadmap because it converts RHDL from "a better HDL" into "the place where hardware IP lives" — a network-effects moat that resists forking the way crates.io resists forking.

---

## 1 — Motivation

Hardware IP distribution has been broken across the entire industry for thirty years. The state of the art is:

- Vendors ship encrypted netlists in zip files with PDFs of timing diagrams and a hand-rolled integration script.
- Open-source IP lives in repositories with no semantic versioning, no published API stability, no compile-time interface checking, and no machine-readable metadata about clock domains, parameter spaces, or timing characteristics.
- Tooling fragments: FuseSoC for the open-source crowd, `.ip` files in Vivado, `.qsys` in Quartus, Microchip's `.cip`, Lattice's various Diamond/Radiant formats, none of them interoperable.
- Versioning is by zip file name. "FIFO_v3_FINAL_FIXED.zip" in someone's Dropbox is a real artifact at real companies in 2026.
- Migration between IP versions is a ritual involving release notes and prayer. There is no compile-time check that "FIFO v1.2.3 still has the same write-port protocol as FIFO v1.2.2."

Software solved this problem fifteen years ago. npm, PyPI, RubyGems, Maven Central, crates.io — all built on the same insight: *a registry plus semver plus a build system that resolves dependencies automatically is a network-effect engine*. The Rust ecosystem in particular shows what this looks like done right: cargo + crates.io + docs.rs + semver + reproducible builds via Cargo.lock + workspace dependencies + features + alternative registries.

**RHDL is uniquely positioned to bring this to hardware** because it is already a Rust crate. The `rhdl-fpga` widget library is already compiled, type-checked, and distributed by cargo today. Every widget already has rustdoc, embedded schematic symbols (via `badascii_doc`), embedded waveforms (via `write_svg_as_markdown`), and tests. The infrastructure is 70% in place; the remaining 30% is the bit-level semver contract, the registry overlay, the reproducibility guarantee, and the certification mechanism.

No other HDL can match this in fewer than five years because no other HDL has cargo underneath it. Chisel has SBT (a Java/Maven world); Bluespec has its own ad-hoc package handling; Verilog/SystemVerilog have nothing; Spade is too young. The window to claim "the place where hardware IP lives" is open and timing-sensitive — every year RHDL waits is a year a competitor could ship a Rust-shaped HDL with the same insight.

---

## 2 — Design goals and non-goals

### Goals

- **`cargo add rhdl-pcie-gen5` works.** Adding a hardware IP dependency is one command. The dependency comes type-checked, with phantom-typed clock domains preserved across the package boundary, with schematic symbols and waveform examples in `cargo doc`.
- **Bit-level semver contract.** A spec defining what counts as a breaking change at the hardware-interface level — separate from but consistent with Rust's normal API semver. A user pinning `rhdl-fifo = "1.2.3"` gets a guarantee about the bit layout of `In`, `Out`, and `State` aggregates, not just about which Rust functions exist.
- **Reproducible Verilog output.** Same source + same compiler version + same lockfile → byte-identical Verilog across machines, operating systems, and clock time. Tested in CI.
- **Curated registry overlay (`registry.rhdl.io`).** A search/browse surface specialized for hardware IP. Filter by category (FIFO, FSM, PHY, DSP), filter by clock domains, sort by certification tier, sort by validation-cluster timing/area on standard parts, view embedded waveforms and FSM diagrams, view "RHDL Certified" badges.
- **"RHDL Certified" mark.** A trademarked certification mechanism that distinguishes IP that has been (a) self-tested, (b) cluster-validated on real FPGA boards, (c) production-tracked through customer attestation.
- **Cross-crate clock-domain consistency.** The phantom-typed `Signal<T, Domain>` machinery already prevents CDC violations within a crate; the package manager extends this guarantee across crate boundaries with no additional user effort.
- **Cargo features as IP variants.** A widget published with `features = ["xilinx-dsp48", "lattice-ebr", "portable"]` lets a user pick the variant that matches their target part. This composes naturally with `vendor-primitive-architecture.md`'s `Target` trait.
- **Lockfile pins everything that affects emitted Verilog.** Compiler version, dependency versions, feature flags, target descriptor (when vendor-primitive plan ships).

### Non-goals (v1)

- **Bitstream reproducibility.** Same Verilog → same bitstream is partly out of RHDL's control (synthesis tools, place-and-route, floorplanning). v1 promises Verilog reproducibility; bitstream reproducibility is a v2 stretch goal contingent on synthesis-tool cooperation.
- **Encrypted IP delivery.** Some commercial IP is delivered encrypted to protect against reverse engineering. Encryption interacts badly with cargo's source-distribution model. v1 ships with source-only delivery; encrypted-delivery is a v3 enterprise feature, not v1.
- **Cross-language IP.** Importing a Verilog module as an RHDL crate is a separate problem (it's a Verilog-import path, not a registry path). Out of scope here; covered by future work in `rhdl-vlog`.
- **Bus-protocol compatibility checking beyond type-equivalence.** Two widgets that both expose `Decoupled<Bits<32>>` are type-compatible; whether their *semantic* protocols are compatible (one expects ready before valid, the other doesn't) is a separate problem. The `stream-bus-architecture.md` plan addresses this for `RCStream`; arbitrary bus protocols are out of scope here.
- **Replacing crates.io.** RHDL crates publish to crates.io exactly like normal Rust crates. `registry.rhdl.io` is an *overlay* providing additional metadata, search, and certification, not a replacement.
- **Operating a vendor IP marketplace.** Premium commercial IP cores live in their vendors' own registries (private cargo registries via `--registry`). The public `registry.rhdl.io` is for open-source IP. Commercial IP marketplace is a separate business artifact, not a v1 deliverable.

---

## 3 — Where this sits

This plan is foundational to the long-term commercial story documented in `bsv-strategy.md` and `chisel-strategy.md`. The package manager is the network-effects moat that turns RHDL from "another HDL" into "the place where hardware IP lives."

Cross-references:

- **`architecture.md`** — the workspace structure already separates crates cleanly; the registry topology in §6 of this document is consistent with the workspace layout. No architectural changes required.
- **`vendor-primitive-architecture.md`** — `Target`-trait-based vendor-primitive selection composes naturally with cargo features. A widget can publish with `features = ["xilinx-dsp48", "lattice-ebr", "portable"]` and consumers select the variant that matches their target.
- **`stream-bus-architecture.md`** — `RCStream<T, F, D>` is the canonical inter-kernel bus and crosses package boundaries naturally because it is type-encoded. Cross-package CDC checking flows through the phantom domain `D`.
- **`rhif-formalization-plan.md`** — the prose spec (Level 1) defines the semantics of every IR opcode; the bit-level semver contract in §4 of this document references the prose spec when defining "what constitutes a behavioral change" of a kernel.
- **`auto-pipelining-plan.md`** — auto-pipelining is per-widget. The package manager preserves auto-pipelining metadata across package boundaries: a downstream consumer of a published widget gets the same auto-pipelining behavior as if the widget were defined locally.
- **`fsm-architecture.md`** — FSM metadata, including the auto-generated state diagram and `#[fsm_invariant]` properties, travels with the crate. The registry's docs surface renders the diagrams the same way `cargo doc` does today.
- **`compile-performance-plan.md`** — Phase 4 (incremental compilation) intersects with the package manager: dependency compilation should hit cargo's existing crate cache. No new mechanism needed; just make sure the per-pass IR caching from Phase 4 respects crate boundaries.
- **`verilog-emission-plan.md`** — reproducible Verilog output is a hard prerequisite for reproducible builds. Phase 1 of the verilog-emission plan must close all sources of nondeterminism in the emission pipeline (sorted iteration order, no thread-id-dependent code, no time-dependent code).

---

## 4 — The bit-level semver contract

This is the technical core of the design. Rust's normal semver covers the surface API (which functions exist, which types they accept). For hardware, that's not enough — two type-compatible interfaces can have *different bit layouts* and produce silently-incompatible silicon.

The bit-level semver contract extends Rust's semver discipline to cover bit layout, behavioral observables, and clock-domain typing. It is enforced in part by the compiler (via existing checks) and in part by the discipline of the publishing author (via prose rules with worked examples).

### 4.1 — Versioning rules for `Digital` types

A `Digital`-derived struct or enum's *bit layout* is part of its public ABI. The rules:

**Adding a field to a public `Digital` struct → MAJOR bump.**
The bit width changes; every downstream consumer that pattern-matches or constructs the struct breaks. There is no `#[non_exhaustive]` escape — `Digital` does not allow "wildcard fields" because the compiler must know the exact bit layout.

**Removing a field → MAJOR.**

**Reordering fields → MAJOR.** The bit layout depends on field order (per `rhdl-bits` packing rules). Even if the type-level API is unchanged, reordering produces incompatible silicon.

**Renaming a public field → MAJOR.** Breaks named field access at every consumer.

**Changing a field's type → MAJOR.** Even if the new type has the same bit width (e.g., `Bits<8>` → `b8` is fine; `Bits<8>` → `Bits<16>` is not; `Bits<8>` → some `Digital` newtype around `Bits<8>` is also a major bump because pattern matching breaks).

**Changing a field's visibility from `pub` to `pub(crate)` → MAJOR.**

**Adding a `#[doc(hidden)]` attribute to a previously visible field → MAJOR for libraries that follow strict semver; MINOR with explicit documentation otherwise. We choose MAJOR for the certified tier and recommend it for everyone.**

**For `Digital` enums:**

- Adding a variant when the enum is `#[non_exhaustive]` → MINOR if discriminator width does not change, MAJOR if it does. The non-exhaustive marker is encoded into the `Digital` derive's metadata so the compiler can enforce this.
- Adding a variant when the enum is not `#[non_exhaustive]` → MAJOR (always).
- Removing a variant → MAJOR.
- Reordering variants → MAJOR if it changes discriminator values (which it almost always does).
- Changing a variant's payload type → MAJOR.
- Adding a payload to a previously-payload-less variant → MAJOR.
- Renaming a variant → MAJOR.

The `#[non_exhaustive]` mechanism is the primary tool for forward-compatible enum evolution. We recommend authors of public `Digital` enums apply it from v1.0.0.

### 4.2 — Versioning rules for widgets

A widget is a struct that implements `Synchronous` (or `Circuit`) plus its kernel function plus its `In` and `Out` types.

**Adding a new public widget → MINOR.**

**Removing a public widget → MAJOR.**

**Changing a widget's `type I = ...; type O = ...; type Kernel = ...;` → MAJOR (always).** Any change to `In`, `Out`, or the kernel signature is a breaking change to the widget's interface.

**Changing the widget's internal sub-circuit composition without changing `In`/`Out`/kernel signature → depends:**

- If the change is *behaviorally observable* (different VCD output for the same input stream) → MINOR if backward-compatible (e.g., a previously-undefined output is now defined; new behavior is a strict refinement), MAJOR if not (e.g., timing changed in a way that affects downstream consumers).
- If the change is *not behaviorally observable* (purely a refactoring; same VCD digest) → PATCH.

The "behavioral observability" bar is enforced by the **Tier 5 VCD digest test** (see CLAUDE.md §5.5). A widget's published behavior is anchored to its committed VCD digest. A digest change is at minimum a MINOR bump; if the new behavior is incompatible with the old, it is a MAJOR.

**Adding generic parameters with defaults → MINOR.**
```rust
// v1.0.0
pub struct FIFO<const N: usize> { ... }
// v1.1.0 — backward-compatible: add a generic with default
pub struct FIFO<const N: usize, const ECC: bool = false> { ... }
```

**Adding generic parameters without defaults → MAJOR** (every consumer must supply the new parameter).

**Removing generic parameters → MAJOR.**

**Changing the bound on a generic parameter:**
- Loosening (e.g., `T: Digital + Default` → `T: Digital`) → MINOR.
- Tightening → MAJOR.

**Changing the clock domain of a port → MAJOR.** The phantom domain is part of the type signature.

### 4.3 — Versioning rules for traits

For user-defined traits with `Digital`-bound associated types or methods on `Digital` values:

- Adding methods with default implementations → MINOR.
- Adding methods without defaults → MAJOR.
- Adding associated types with defaults → MINOR.
- Adding associated types without defaults → MAJOR.
- Adding required supertraits → MAJOR.
- Removing items → MAJOR.

These mirror standard Rust semver; the reason to restate them is that some HDL communities have intuitions imported from C/Verilog where "adding a function is always backward-compatible," which is wrong for Rust traits because users may have provided their own implementations.

### 4.4 — Versioning rules for clock domains and `Target` profiles

**Public `Domain` types (e.g., `pub struct Red;` at crate root) are part of the API.** Removing or renaming a public domain → MAJOR.

**Adding a new public `Domain` type → MINOR.**

**Changing a `Target` profile's emitted Verilog under the same `Descriptor::hdl_for(&target)` call → MINOR if the silicon behavior is unchanged (purely a stylistic change to the Verilog output), MAJOR if behavior changes.**

**Adding support for a new `Target` → MINOR (existing consumers' targets are unaffected).**

**Removing support for a `Target` → MAJOR for users of that target.**

### 4.5 — Versioning rules for the implicit clock-reset interface

The `ClockReset` type and its semantics are part of `rhdl-core`. Changes to `ClockReset` are versioned via the `rhdl` meta-crate's semver, not per-widget. A widget that uses `cr.reset.any()` in its kernel is implicitly tied to whatever `rhdl` version it was built against; the lockfile pins this.

### 4.6 — The "compatibility matrix" worked example

To make the rules concrete, here is the canonical worked example for a FIFO:

```rust
// FIFO v1.0.0 — initial release
#[derive(Synchronous, SynchronousDQ)]
pub struct FIFO<T: Digital, const N: usize> { /* ... */ }

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct In<T: Digital> {
    pub data: T,
    pub write: bool,
    pub read: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct Out<T: Digital> {
    pub data: T,
    pub full: bool,
    pub empty: bool,
}
```

| Change | Old version | New version | Bump |
|---|---|---|---|
| Add `pub overflow: bool` to `Out` | 1.0.0 | 2.0.0 | MAJOR (bit layout) |
| Change `pub write: bool` to `pub write: Option<T>` in `In` | 1.0.0 | 2.0.0 | MAJOR (type change) |
| Add `Default` derive to `FIFO` | 1.0.0 | 1.1.0 | MINOR (additive) |
| Fix bug where empty was asserted one cycle late | 1.0.0 | 2.0.0 | MAJOR (VCD digest changes; behavioral break) |
| Improve emitted Verilog formatting (no behavior change) | 1.0.0 | 1.0.1 | PATCH (VCD digest unchanged) |
| Add a vendor-primitive `Target` for Xilinx Block RAM | 1.0.0 | 1.1.0 | MINOR (additive `hdl_for(&target)` overload) |
| Add `const ECC: bool = false` generic parameter | 1.0.0 | 1.1.0 | MINOR (default-valued generic) |
| Remove the `N` generic parameter (hardcode to 16) | 1.0.0 | 2.0.0 | MAJOR (generic removed) |
| Change `Red` clock domain to a new `RedV2` domain on the write port | 1.0.0 | 2.0.0 | MAJOR (type change in port) |

This matrix is normative. Authors publishing certified IP must apply it; the registry surface displays the version-bump rationale in the changelog view.

### 4.7 — Enforcement

Three layers:

**Compiler-enforced (free).** Rust's existing semver discipline catches type-level breaks at compile time. Adding a non-default generic parameter breaks compilation at the consumer; renaming a field breaks pattern matching; removing a variant breaks exhaustive match. These produce normal Rust errors.

**Tooling-enforced (Phase 1 deliverable).** A `cargo rhdl semver-check` subcommand that compares two crate versions and reports semver violations. Specifically:

- Compares the bit width of every public `Digital` type. A width mismatch with the same major version is a hard error.
- Compares the discriminator width of every `Digital` enum.
- Compares the clock domain of every public widget port.
- Compares VCD digests for every published widget against its committed Tier-5 digest test. A digest change with the same major version + minor version is a hard error.
- Output is a `miette` diagnostic in the existing project style.

**Author discipline (forever).** The behavioral semver rules (4.2's "behaviorally observable" distinction, 4.4's "stylistic vs. behavioral" distinction) cannot be fully mechanized. The published spec — this document — is the contract; the registry's certification process audits it.

---

## 5 — Reproducibility contract

### 5.1 — What we promise

Same source + same compiler version + same lockfile + same target descriptor → **byte-identical Verilog output**, on any machine, any operating system, any time.

This is enforced in CI via a `reproducibility-check` job that performs two clean builds in parallel and `diff`s the emitted Verilog. Any divergence is a P0 bug.

### 5.2 — What we don't promise

- **Same Verilog → same bitstream.** Synthesis tools (Vivado, Quartus, Diamond, Yosys, Radiant) have their own determinism stories ranging from "bit-reproducible with a flag" (Vivado with `-no_timing_driven`) to "approximately reproducible" to "non-deterministic by default." v1 of the package manager promises Verilog reproducibility; bitstream reproducibility flows through whatever the synthesis tool offers and is documented per-target.
- **Same source across compiler versions.** The compiler version is part of the lockfile. Upgrading rhdl is a deliberate act and may cause Verilog output to change. The verilog-emission plan tracks this.
- **Same source across feature-flag combinations.** Features are part of the lockfile. A widget compiled with `features = ["xilinx-dsp48"]` produces different Verilog than one without, by design.
- **Same source across target descriptors.** A widget compiled for Xilinx UltraScale produces different Verilog than the same widget compiled for portable output, by the design of `vendor-primitive-architecture.md`. The target descriptor is part of the lockfile.

### 5.3 — Sources of non-determinism to eliminate

Implementation work in `rhdl-core`:

- **HashMap iteration order.** Already a known issue per `rule-architecture.md` §18 ("Determinism in macro-expansion order"). Replace `HashMap` with `BTreeMap` (or a deterministic-iteration-order hash map) wherever iteration appears in the lowering pipeline.
- **Thread-ID-dependent code.** Compiler passes that consult `std::thread::current().id()` produce non-reproducible output. Audit and remove.
- **Time-dependent code.** Compiler passes that embed timestamps in the output produce non-reproducible output. Audit and remove (or use a build-time-injected fixed timestamp — see §5.4).
- **Float NaN handling in pass-internal code.** Some hashing routines that incidentally process floats can produce non-deterministic output across architectures. Audit; the compiler should not depend on float operations internally.
- **Symbol-table ordering** in the post-elaboration phase. Currently there is some ordering by hash-set iteration. Replace with sorted iteration.

This work is approximately one engineer-month, mostly mechanical. It belongs in `compile-performance-plan.md` Phase 0 (profiling/baseline) extended with a "determinism baseline" that gates Phase 1.

### 5.4 — Reproducibility metadata in the emitted Verilog

Every emitted Verilog file gets a header comment block:

```verilog
// Generated by rhdl 1.4.7 (Apache-2.0)
// Source: my-crate v0.3.2 (sha256:abc123...)
// Lockfile digest: sha256:def456...
// Target: portable | xilinx-ultrascale | lattice-certuspro
// Features: xilinx-dsp48,ecc
// Reproducibility: byte-identical guaranteed against the inputs above
```

This makes audit trivial. Two emitted files with matching headers must be byte-identical; any divergence is a P0 reproducibility bug.

---

## 6 — Registry topology

### 6.1 — The hybrid model

RHDL crates are published to **crates.io** exactly like normal Rust crates. There is no separate `crates.rhdl.io`; that would fragment the ecosystem and force authors to publish twice.

`registry.rhdl.io` is a **curated metadata overlay** that points back to crates.io entries while adding hardware-specific metadata, search, and certification. Conceptually:

```
crates.io                           registry.rhdl.io
─────────                           ──────────────────
my-fifo v1.2.3 ──────────────────►  my-fifo (hardware overlay)
   ├── .crate file                     ├── certification: tier-2
   ├── Cargo.toml                      ├── category: fifo, sync, fpga
   ├── lib.rs                          ├── clock-domains: 1
   └── docs (built by docs.rs)         ├── targets: portable, xilinx, lattice
                                       ├── timing on Artix-7: 200 MHz
                                       ├── area on Artix-7: 124 LUT, 8 FF
                                       ├── timing on iCE40: 80 MHz
                                       ├── waveform examples (rendered)
                                       ├── FSM diagrams (rendered)
                                       ├── changelog with semver rationale
                                       └── validation-cluster history
```

The user does `cargo add my-fifo` exactly as today. The hardware overlay is a website where they go *before* doing `cargo add` to discover, compare, and verify IP — and that the certification mark, validation reports, and timing/area data display from.

### 6.2 — The overlay-metadata format

A crate participates in the overlay by including a top-level `rhdl-metadata.toml` file. The overlay's ingestion pipeline picks this up, validates it against a schema, runs the certification checks (per §7), and publishes the metadata. The crate itself is unchanged.

```toml
# rhdl-metadata.toml at crate root
[package]
category = "fifo"             # one of a fixed list
subcategory = "synchronous"   # subcategorization within the category
hardware-tier = "stable"      # alpha | beta | stable | mature

[interfaces]
# Names of public widgets and their exposed buses.
widgets = ["FIFO", "AsyncFIFO"]

[clock-domains]
# How many independent domains the public widgets carry across all variants.
"FIFO" = 1
"AsyncFIFO" = 2

[targets]
# Which Target profiles the crate provides hdl_for() implementations for.
default = "portable"
supported = ["portable", "xilinx-ultrascale", "lattice-certuspro", "ice40"]

[validation]
# Pointers to the committed validation artifacts.
vcd-digests = ["doc/fifo.md", "doc/async_fifo.md"]
fsm-diagrams = ["doc/sync_fifo_fsm.md"]
synthesis-reports = "validation/"  # per-target timing/area JSON
```

The overlay reads this metadata at ingestion time, links the artifacts, and renders the registry page. Authors who don't include this file still appear on crates.io but don't get the hardware overlay treatment — they appear as "unverified" in registry.rhdl.io searches.

### 6.3 — Operational footprint

`registry.rhdl.io` is a static-site-generator-driven website with a small ingestion daemon:

- Ingestion daemon polls crates.io for new versions of `[hardware-overlay]` opted-in crates; for each, downloads the crate, validates `rhdl-metadata.toml`, compiles the rustdoc with the hardware-rendering plugin (per §7), runs synthesis on the validation cluster (per §8), and updates the static site.
- Static site is hosted on commodity object storage with a CDN.
- Total operating cost at modest scale (1000 crates, 100 new versions/week, validation cluster runs): ~$50k/year.

This is a small operations problem, not a research problem.

### 6.4 — Private and enterprise registries

Cargo supports alternative registries via `--registry` and the `[registries]` config section. Commercial IP vendors can run `--registry` endpoints serving their own crates with their own access control. This is exactly the same pattern as private crates.io alternatives in Rust software (Cloudsmith, Artifactory, JFrog, custom).

The hybrid model allows three deployment patterns:

**Public open-source IP.** Published to crates.io, indexed by registry.rhdl.io, certified through the public process.

**Enterprise internal IP.** Published to a company's private cargo registry, optionally indexed by an internal mirror of registry.rhdl.io for browse/discovery within the company.

**Commercial IP.** Published to a vendor's private cargo registry (e.g., `crates.acme-ip.com`), with their own access control and licensing. Vendors may optionally publish metadata-only entries to registry.rhdl.io that link to their commercial registry, similar to how npm has private packages.

---

## 7 — The hardware-aware documentation surface

`docs.rs` already renders RHDL crate documentation correctly because the existing rustdoc-with-include-str pattern (per CLAUDE.md §6 Layer A) just works. Schematic symbols render via inline SVG, waveforms render via the `write_svg_as_markdown` machinery, FSM diagrams render via the existing FSM derive plumbing.

The remaining work is two pieces:

### 7.1 — A custom rustdoc plugin (`mdbook-rhdl`-style for rustdoc)

Currently `mdbook-rhdl` is the book preprocessor. The analog for rustdoc is a small RHDL-specific rustdoc post-processor that:

- Recognizes `Synchronous` / `Circuit` impls and renders a "Hardware" section in the rendered docs.
- Surfaces the `In` / `Out` types with their bit widths, field-by-field.
- Surfaces the public clock domains in a port-like view.
- Links related widgets within the crate (e.g., a top-level FIFO links to its sub-widgets).
- Renders the `cargo rhdl semver-check` baseline so a downstream user sees "this widget is bit-stable since v1.0.0."

This is invoked by docs.rs the same way the existing badascii-doc rendering is. It does not require docs.rs modifications; it's a normal rustdoc plugin published as a separate crate.

### 7.2 — registry.rhdl.io's rendering pipeline

registry.rhdl.io hosts the same content but with cross-crate features:

- Search by category, by clock-domain count, by certification tier, by target support.
- Compare two crates side-by-side (timing, area, interface).
- Browse by validation-cluster results (e.g., "all tier-2-certified FIFOs for Xilinx UltraScale ranked by area").
- Show the dependency graph: which other crates depend on this one, which crates does it depend on. Same as crates.io's reverse-dependency view but hardware-aware.

The rendering is done at ingestion time, statically. No live database queries.

---

## 8 — The "RHDL Certified" mark

A trademarked certification mark with three tiers in increasing rigor.

### 8.1 — Tier 1 — Self-Certified

Any crate that:
- Has a valid `rhdl-metadata.toml`.
- Compiles cleanly with the current stable rhdl release.
- Has all tests passing in its own CI (the badge consumes the GitHub Actions / GitLab CI status badge).
- Has the five-tier validation stack from CLAUDE.md present (kernel test, iterator sim, HDL snapshot, iverilog round-trip, VCD digest).

Tier 1 is automatic on opt-in. The author opts in by including `rhdl-metadata.toml`; the overlay validates the criteria and stamps the mark.

**Rationale: low bar, large funnel.** Tier 1 says "this is a real RHDL crate that follows the conventions." It's the minimum bar to appear in registry.rhdl.io searches. It doesn't certify behavior; it certifies hygiene.

### 8.2 — Tier 2 — Cluster-Certified

A Tier-1 crate that has additionally been built and run on the **RHDL validation cluster** — a physical infrastructure of FPGA boards (Xilinx UltraScale, Lattice CertusPro, iCE40, OpalKelly XEM7010, Microchip PolarFire) that runs the crate's tests against real silicon, measures timing/area/power, and publishes the report.

Tier 2 is automatic on the validation cluster's success. Authors don't apply; the cluster runs every Tier-1 crate periodically and stamps Tier-2 on the ones that pass. Failure is logged and visible (so authors get feedback) but doesn't penalize the crate.

**The validation cluster is the moat from `bsv-strategy.md` and `chisel-strategy.md`'s strategic context.** It is a physical capital asset that no fork can replicate in 2 days.

### 8.3 — Tier 3 — Production-Tracked

A Tier-2 crate that has additionally been **deployed in a real production design that's in the field** — customer-attested, with a published case study, with attested production volume.

Tier 3 is opt-in by the customer (the company running the production design). It requires legal sign-off and a published case study. It is the highest mark and the strongest market signal.

**Tier 3 is the marketing payoff of the entire mechanism.** It is what enterprise procurement processes look for.

### 8.4 — Mark licensing and trademark protection

The "RHDL Certified" word mark and its associated logo are trademarked (per the business strategy in the project root). The license to display the mark is granted automatically by the overlay based on the tier criteria above. The mark is revoked if a crate fails the criteria on a re-validation cycle.

This creates a strong incentive for authors to maintain their crates: a crate that loses its Tier-2 mark because of a regression is publicly visible.

### 8.5 — Trademark scope

The mark covers RHDL specifically. It does not extend to:
- Forks of RHDL using a different name.
- Other HDLs claiming "RHDL-compatible" status.
- IP that runs on RHDL but isn't certified.

The mark is the legal moat; it does not compound (as compounding moats do — see the model+data flywheel) but it is *permanent* in a way compounding moats are not.

---

## 9 — Cross-crate clock-domain consistency

The phantom-typed `Signal<T, Domain>` machinery already prevents CDC violations within a crate. The package manager extends this guarantee across crate boundaries with **no additional user effort**, because Rust's type system already does the work.

### 9.1 — Domains as types in the public API

A crate that wants to expose a clock-domain-aware widget exports its domain types as part of its public API:

```rust
// crate-a/src/lib.rs
pub struct Red;
pub struct Blue;

impl Domain for Red { /* ... */ }
impl Domain for Blue { /* ... */ }

pub struct DualClockFIFO<T: Digital, W: Domain, R: Domain> { /* ... */ }
```

A consumer that uses `DualClockFIFO<u32, crate_a::Red, crate_a::Blue>` is type-safe by Rust's normal rules.

### 9.2 — Domain identity across crates

Two crates can each define their own private domains. `crate_a::Red` and `crate_c::Red` are distinct types (different fully-qualified paths) and the type system treats them correctly. A user wiring across crates A and C must explicitly choose how to bridge them — the same way they would within a single crate, via existing `cdc::*` widgets.

This means: **there is no global domain registry**. Domains are per-crate; bridging is explicit. This is by design, mirroring how Rust's type system handles trait orphan rules.

### 9.3 — The "common domain" pattern for libraries

A common pattern in Rust libraries: define commonly-used types in a sibling crate to allow multiple downstream crates to share them. The same applies here. A `rhdl-domains-common` crate could define commonly-used domains (`Red`, `Blue`, `Green`, etc.) and other crates could import from it. The current `rhdl-core` already exposes the standard color domains; extending this is the path forward.

### 9.4 — Domain compatibility in semver

A crate that exposes `pub struct Red;` as a domain is committed to that name being part of the public API. Removing or renaming `Red` is a MAJOR bump (per §4.4). Adding a new domain is MINOR.

---

## 10 — Lockfile semantics

Cargo.lock today pins crate versions. For RHDL, the lockfile additionally pins:

**The compiler version.** The `rhdl` meta-crate's exact version. Already pinned by Cargo.lock; just ensure it's the version used during emission.

**Feature flag combinations per crate.** Cargo already does this; no new mechanism needed.

**Target descriptor.** When the vendor-primitive plan ships, the target descriptor (which selects which `hdl_for(&target)` is invoked) becomes part of the lockfile. Two builds with different targets produce different Verilog by design, so the target must be pinned.

**Synthesis tool versions (informational).** When the user wants bitstream reproducibility (v2), the lockfile records which synthesis tool versions were used. RHDL doesn't run the synthesis tool, so this is informational, not authoritative — it's the user's responsibility to use the same tool version.

The lockfile is hashed and the hash appears in the emitted Verilog header (per §5.4). Two builds produce byte-identical output if and only if their lockfiles match.

---

## 11 — Phasing

### Phase 1 (3-6 months) — The semver contract and reproducibility

Deliverables:
- This document, in final form, accepted as the project's normative bit-level semver contract.
- `cargo rhdl semver-check` subcommand implemented in `rhdl-cli`.
- All sources of non-determinism in the emission pipeline closed (per §5.3).
- CI reproducibility-check job added.
- VCD digest tests in every existing widget (most already have them; backfill any that don't, per CLAUDE.md §5.5).

Critically, this phase requires **no new infrastructure**. It's all within the existing repo. The cost is engineering discipline, not capital.

### Phase 2 (6-12 months) — The overlay and Tier 1 certification

Deliverables:
- registry.rhdl.io launched as a static site.
- Ingestion daemon for crates.io, processing crates with `rhdl-metadata.toml`.
- Tier-1 certification automatic on opt-in.
- Custom rustdoc plugin shipped (per §7.1).
- Cross-crate clock-domain checking validated end-to-end (already free; tests confirm).
- One worked example: a third-party crate (perhaps from a Berkeley grad student or a partner company) published with full overlay metadata.

The infrastructure cost in this phase is the registry's hosting (~$50k/year) and the rustdoc plugin engineering. No physical capital.

### Phase 3 (12-24 months) — The validation cluster and Tier 2 certification

Deliverables:
- Validation cluster physical infrastructure stood up: Xilinx UltraScale, Lattice CertusPro, iCE40, OpalKelly XEM7010, Microchip PolarFire boards.
- Synthesis tool licensing acquired.
- Cluster orchestration software written (probably ~3000 LOC in Rust, scheduling synthesis jobs and collecting results).
- Tier-2 certification automatic.
- Per-target timing/area/power reports rendered on registry.rhdl.io.
- Cross-vendor synthesis comparison view in the overlay.

This is the capital-intensive phase. Budget ~$300k-$500k for the boards, licenses, and the rack. Plus engineering for the orchestration software.

### Phase 4 (24+ months) — Tier 3 certification, enterprise features

Deliverables:
- Tier-3 production-tracked process formalized.
- First customer Tier-3 case study published.
- Enterprise registry (private namespaces, access control) — not because we host it, but because cargo's existing alternative-registry mechanism handles it; we just have to document and bless the pattern.
- Bitstream reproducibility (where vendor toolchain allows).
- Encrypted IP delivery (commercial IP vendors).

---

## 12 — Risks and open questions

**Reproducibility-or-die.** If reproducibility breaks for any reason — a non-determinism bug in the compiler, a thread-ID-dependent code path, a HashMap iteration order leak — the whole certification mechanism collapses. This requires permanent CI vigilance. Mitigation: the reproducibility-check CI job is mandatory; any regression is P0.

**The semver enforcement gap.** §4.7 acknowledges that some semver rules are author discipline, not mechanically enforced. There is real risk of authors publishing under-bumped versions and breaking downstream consumers. Mitigation: the registry's certification re-validation on each new version catches MAJOR-level breaks (the bit width changes; the cluster fails). MINOR-level "behavioral observability" breaks are caught only by the VCD digest test, which authors might skip. Mitigation: the registry's Tier-1 stamp requires the digest test to be present and passing. No digest test → no Tier-1 stamp.

**Cargo's monorepo-vs-multi-repo bias.** Cargo strongly assumes per-crate-per-repository or workspace-per-repository. Hardware IP often comes in larger collections (e.g., "the Xilinx PCIe IP suite" with shared internal types). Mitigation: cargo workspaces handle this fine; the overlay treats workspaces as collections of related crates with shared metadata. Document the recommended structure clearly.

**Trademark dilution.** If "RHDL Certified" gets applied liberally, it loses meaning. Mitigation: the three-tier structure (with Tier-2 requiring cluster validation) keeps the bar clear. Tier-2+ marks are gated by physical infrastructure, which is fork-resistant.

**Dependency on the `rhdl` meta-crate's stability.** Every crate in the registry depends on `rhdl`. A breaking change in `rhdl` invalidates the entire registry's certification status. Mitigation: `rhdl` follows strict semver; breaking changes are major bumps; the registry tracks the "last known good rhdl version" per crate.

**Operating registry.rhdl.io is a real commitment.** $50k/year in hosting, plus on-call, plus moderation. If RHDL is run as an open-source-with-commercial-overlay business, this lives under the commercial entity. If RHDL is run as a foundation, it lives under the foundation. The decision is part of the broader business strategy in `bsv-strategy.md` and is not strictly an architectural concern, but operational continuity matters.

**Encrypted-IP impedance mismatch.** Commercial IP is often delivered encrypted to protect against reverse engineering. cargo's source-distribution model is incompatible with encrypted delivery. v1 punts on this; v3+ requires a separate distribution mechanism (probably a per-vendor private cargo-compatible registry that delivers encrypted netlists with a runtime decryption hook). This is an open architectural question for the commercial-IP-marketplace business artifact.

**Forking of registry.rhdl.io.** Open-source overlay metadata is forkable. A competitor could spin up `registry.competitor.io` with the same data. Mitigation: the trademark is the moat (per §8.5); the data flywheel from Tier-2 validation is the moat; the brand recognition compounds. Forking the *contents* doesn't fork the *trust*.

**Cargo limitations for hardware.** Cargo lacks some features the hardware world wants — version-range solving for binary compatibility (vs. source compatibility), bit-level interface-stability annotations, optional dependencies with runtime selection. We work around these by (a) documenting conventions, (b) using `cargo rhdl semver-check` as an external linter, (c) accepting some imprecision. If specific cargo features become blocking, contributing upstream to cargo is an option.

**Versioning of the metadata schema itself.** `rhdl-metadata.toml` will evolve. Versioning the schema is necessary. v1 of the schema is documented here; future versions are spec'd in dedicated docs.

---

## 13 — Validation

How we know the package manager works:

**Test 1 — A third-party crate gets published and consumed.** Before declaring Phase 2 complete, at least one external (non-RHDL-team) author publishes a hardware crate with full overlay metadata, gets Tier-1 certified, and is consumed by an unrelated downstream project that does `cargo add` and depends on it. End-to-end working.

**Test 2 — The semver linter catches a real break.** A canary widget is intentionally regressed (a field is added without a major bump); `cargo rhdl semver-check` catches it; the registry refuses to ingest the bad version.

**Test 3 — Reproducibility CI catches a non-determinism regression.** A canary commit introduces a HashMap-iteration-order dependency; CI's reproducibility check catches it within hours.

**Test 4 — A Tier-2 crate's cluster certification is meaningful.** Before declaring Phase 3 complete, a Tier-2 crate is referenced in a third-party blog post or paper as "we picked this crate over an alternative because it had Tier-2 certification."

**Test 5 — A Tier-3 customer attestation is published.** Before declaring Phase 4 complete, the first customer case study is published, citing concrete production volume and a specific certified crate.

---

## 14 — Crate organization

The package manager work touches several crates:

```
rhdl-metadata/                    new crate; defines rhdl-metadata.toml schema, parser
rhdl-cli/                         existing crate; gets `cargo rhdl semver-check` subcommand
rhdl-rustdoc-plugin/              new crate; the docs.rs hardware-rendering plugin
registry-server/                  new repo (separate from rhdl/); the registry.rhdl.io site generator
validation-cluster/               new repo (separate); the orchestration software
```

The first three are part of the rhdl workspace. The last two are operational infrastructure, separately hosted.

---

## 15 — References

1. Cargo Reference — Manifest Format. https://doc.rust-lang.org/cargo/reference/manifest.html
2. Cargo Reference — Specifying Dependencies. https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html
3. Cargo Reference — Alternative Registries. https://doc.rust-lang.org/cargo/reference/registries.html
4. The Rust API Guidelines, "Future-Proofing." https://rust-lang.github.io/api-guidelines/future-proofing.html
5. SemVer specification. https://semver.org/
6. Rust RFC 1105 — API Evolution. https://rust-lang.github.io/rfcs/1105-api-evolution.html
7. crates.io operational documentation. https://crates.io/policies
8. docs.rs documentation. https://docs.rs/about
9. FuseSoC — the existing open-source HDL package manager. https://github.com/olofk/fusesoc
10. Apache 2.0 trademark policy. https://www.apache.org/foundation/marks/
11. CLAUDE.md §6 — module-rustdoc contract for widgets.
12. `architecture.md` §3 — crate dependency graph; the package manager preserves the existing graph.
13. `vendor-primitive-architecture.md` — `Target` profiles and `Descriptor::hdl_for(&target)`.
14. `stream-bus-architecture.md` — `RCStream<T, F, D>` as the canonical inter-kernel/inter-crate bus.
15. `verilog-emission-plan.md` — emission determinism requirements.
16. `compile-performance-plan.md` Phase 0 — determinism baseline.
17. `bsv-strategy.md` and `chisel-strategy.md` — the strategic context for the certification mark.

---

## 16 — Decisions captured

These decisions are captured as part of accepting this document. They are normative; revisiting them requires sign-off per CLAUDE.md §0.

1. **The bit-level semver contract is normative.** §4 governs every published RHDL crate's versioning. `cargo rhdl semver-check` enforces what's mechanizable; author discipline (audited by the certification process) enforces the rest.

2. **Reproducibility is a project-wide invariant, not a per-feature opt-in.** Same source + same compiler version + same lockfile + same target descriptor produces byte-identical Verilog, in CI, forever. Any regression is P0.

3. **Hardware IP is published to crates.io, not a separate registry.** The hybrid model preserves the existing Rust ecosystem alignment; registry.rhdl.io is a metadata overlay with cross-crate features, not a competing registry.

4. **The "RHDL Certified" mark is a trademarked, three-tier mechanism.** Tier 1 is hygiene; Tier 2 is silicon validation; Tier 3 is production attestation. The mark is the legal moat; the validation cluster is the capital moat; the data flywheel is the compounding moat.

5. **Phantom clock-domain typing extends across crate boundaries with no additional mechanism.** Domain types are normal Rust types; the existing type system handles them. Cross-crate domain bridging is explicit via `cdc::*` widgets, mirroring within-crate practice.

6. **Cargo features encode `Target` variants.** A widget published with `features = ["xilinx-dsp48", "portable"]` lets the consumer pick the variant. This composes with the `Target` trait without new mechanism.

7. **The lockfile pins everything that affects emitted Verilog.** Compiler version, dependency versions, feature flags, target descriptor. Two builds with matching lockfiles produce byte-identical Verilog by definition.

8. **Tier 1 and Tier 2 certification are automatic; Tier 3 is opt-in by the customer.** Authors don't apply for marks; the registry stamps based on objective criteria. This minimizes friction and avoids the perception of gatekeeping.

9. **Encrypted IP delivery is out of scope for v1.** v1 is source-distribution like cargo. Encrypted commercial IP is a separate distribution mechanism (per-vendor private registries with runtime decryption hooks), addressed in v3+.

10. **The package manager is the highest-leverage feature on the roadmap.** Phasing prioritizes it accordingly: Phase 1 (semver + reproducibility) ships within 3-6 months as a no-new-infrastructure deliverable; Phase 2 (overlay + Tier 1) within 12 months; Phase 3 (validation cluster + Tier 2) within 24 months. This phasing is fastest-to-value because each phase delivers usable adoption signal independently.
