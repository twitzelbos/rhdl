# Compile Performance Plan — Making the RHDL Compiler Fast

A plan for reducing the wall-clock time of RHDL compilation by an order of magnitude or more, drawn from a survey of the actual compiler code in `rhdl-core/src/compiler/`. The bottlenecks are concrete, measurable, and addressable in a phased program of work that reuses the existing pass infrastructure rather than rewriting it.

This plan is not a feature plan. It does not change what the compiler does, only how fast it does it. Functional behavior is preserved at every step; the snapshot tests are the safety net.

---

## 1 — Motivation

Compile time is starting to matter for three concrete reasons.

**The corpus has grown.** The widget library is over 35 widgets shipped, with a 27-widget FSM corpus that runs a snapshot regression test on every build. Each widget exercises `cargo check`, `cargo test` (which includes Tier 1–5 validation), and `iverilog` round-trip. Iteration is no longer "compile one widget"; it's "compile the whole library plus run the corpus."

**The agent-driven workflow depends on fast feedback.** The thesis from `manifesto.md` — LLMs benefit disproportionately from tight error-feedback loops — is empirically only true if those loops are tight. A 30-second `cargo check` cycle is fine for a human; an LLM agent driving 50-100 iterations per task pays the cost in wall-clock seconds × iterations. A 5-second `cargo check` is the difference between an agent finishing a kernel in two minutes and finishing it in twenty.

**The competitor HDLs are not getting slower.** Chisel + FIRRTL builds quickly because Scala's incremental compilation amortizes most work; SpinalHDL similarly. Yosys-based open-source flows have multi-second turnaround on small designs. RHDL's compile time is currently the slowest among modern HDLs in this class for non-trivial kernels — a gap that will become visible as soon as third-party benchmarks appear.

The right time to attack compile performance is *now*, before the codebase has grown to the point where the cost of refactoring exceeds the benefit. Right now the IR data structures and pass infrastructure are still clean enough to mechanically transform. If we wait until the compiler has 30+ passes per stage, more contributors, and more downstream consumers, the same refactoring becomes 10x more expensive.

The good news is that the compiler is well-structured. The bottlenecks are obvious, the fixes are mechanical, and the existing pass framework is the right place to apply them.

---

## 2 — What the code reveals

A survey of the compiler stage drivers and pass infrastructure surfaces four primary bottlenecks. Each is documented with concrete code references.

### 2.1 Pass-by-value moves the entire Object every call

The Pass trait is defined as:

```rust
pub trait Pass {
    fn run(input: Object) -> Result<Object, RHDLError>;
    fn description() -> &'static str;
}
```

Every pass takes `Object` by value and returns `Object` by value. The compiler's pass driver looks like:

```rust
fn wrap_pass<P: Pass>(obj: Object) -> Result<Object> {
    debug!("Running Stage 1 Compiler Pass {}", P::description());
    let obj = P::run(obj)?;
    debug!("Pass complete - checking symbol table");
    let obj = SymbolTableIsComplete::run(obj)?;
    Ok(obj)
}
```

Each call moves Object through three function boundaries: `wrap_pass` → `P::run` → `SymbolTableIsComplete::run` → back to `wrap_pass`. For passes that do not modify the Object (the common case during fixed-point convergence), this is wasted work. For a kernel whose Object is megabytes in size, it's wasted work *measured in milliseconds*.

The deeper issue: many of the IR's nested `Vec<...>` collections are heap-allocated. A pass that copies-and-mutates an Object cascades to a clone of every nested allocation. Returning a modified Object means rebuilding the entire heap-allocated tree.

**Fix candidate:** change `fn run(input: Object) -> Result<Object, _>` to `fn run(input: &mut Object) -> Result<(), _>`. Eliminates the move per pass call. Mechanical refactor.

### 2.2 Hash-based fixed-point detection re-hashes the entire Object every iteration

The stage-1 driver runs passes in a fixed-point loop:

```rust
let mut hash = obj.hash_value();
loop {
    obj = wrap_pass::<RemoveUnneededMuxesPass>(obj)?;
    obj = wrap_pass::<RemoveExtraRegistersPass>(obj)?;
    // ... 12 more passes ...
    let new_hash = obj.hash_value();
    if new_hash == hash { break; }
    hash = new_hash;
}
```

`hash_value()` on Object walks the entire IR and hashes every field. For a kernel with thousands of slots, this is non-trivial; doing it on every loop iteration is significant.

The deeper issue: the loop runs *every pass* on every iteration, even when only one pass will produce a change. After two iterations, typically only one or two passes are actually still doing useful work; the rest are no-ops, but each runs to completion and is re-hashed.

**Fix candidate:** add a `bool changed` return from each pass; the loop converges as soon as no pass returns `true` for an entire iteration. Skip `hash_value()` entirely. Per-pass dirty-bit tracking is O(1) per pass instead of O(N) per iteration.

### 2.3 `SymbolTableIsComplete` runs after every pass

The pass driver wraps every pass with a `SymbolTableIsComplete::run(obj)` invariant check:

```rust
fn wrap_pass<P: Pass>(obj: Object) -> Result<Object> {
    let obj = P::run(obj)?;
    let obj = SymbolTableIsComplete::run(obj)?;  // ← every pass
    Ok(obj)
}
```

The implementation walks every slot in the Object via `visit_object_slots`, checking each is registered in the symbol table. For a 14-pass loop iteration, this is 14 full-Object walks just for the invariant check.

The motivation is correctness — passes that drop a slot from the symbol table are caught early. But this is a development-time concern. In a production build with snapshot-tested passes, the invariant has been verified once and doesn't need to run after every pass; it only needs to run after each *stage*.

**Fix candidate:** gate `SymbolTableIsComplete` behind a debug-build cfg flag. Run it once per stage in release builds. Saves ~14 N-walks per iteration in stage 1, ~17 in stage 2, ~16 in stage 3.

### 2.4 Two fixed-point loops in stage 1, both running the full pass set

Stage 1 actually has *two* fixed-point loops:

```rust
let mut hash = obj.hash_value();
loop { /* 8 simplification passes */ ... }

obj = CheckClockDomain::run(obj)?;

let mut hash = obj.hash_value();
loop { /* 14 fuller-optimization passes */ ... }
```

Both loops run their respective pass sets to convergence. The second loop's pass set is a *superset* of the first loop's pass set, plus more aggressive transformations. So the work in the first loop is re-done in the second loop with potentially-converged data.

**Fix candidate:** can the first loop be eliminated entirely, or can its passes be moved into a guarded subset of the second loop's iteration? The passes in the first loop seem to be the cheap "remove unused stuff" cleanup; running them once before the second loop doesn't gain much over just running them once *inside* the second loop with a "skip if not at first iteration" guard. Worth measuring before changing.

### 2.5 Other observed bottlenecks

Less load-bearing but still real:

- **`derive(Clone, Hash)` on Object types.** Both traits walk every field. `Clone` is the cost we're trying to avoid; `Hash` we just discussed. Custom impls could skip irrelevant fields (source positions, comments) for hashing, reducing the work even when hashing is necessary.
- **`derive(Debug)`.** Used by `log::debug!()` calls in pass wrappers. Even when log level is off, the debug-formatted string is constructed. Should be replaced with lazy logging or compiled out in release builds.
- **`Vec<OpCode>` in tight loops.** Most pass code iterates over `obj.ops: Vec<OpCode>`. With ~100-1000 ops per kernel, this is fast individually but multiplies across passes × iterations.
- **`Vec`-of-`Vec` for case tables, op arguments, etc.** Cloning is expensive. Persistent data structures (`im::Vector`, `rpds::Vector`) would clone in O(1).
- **proc-macro compile time.** `rhdl-macro` and `rhdl-macro-core` are heavy proc-macro crates. proc-macros recompile every time a downstream crate changes, which means every widget rebuild triggers macro reload.
- **`expect_test` snapshot machinery.** Loads file from disk, parses, diffs against expected. For a 27-widget corpus, this is ~27 file loads per `cargo test` invocation.
- **iverilog round-trip overhead.** External-process fork/exec per widget × cycle to start iverilog × per-test simulation. Even fast simulations have ~100ms of overhead per call.

---

## 3 — Categories of speedup

The bottlenecks fall into four categories, each with distinct effort and payoff.

| Category | Examples | Effort | Speedup |
|---|---|---|---|
| Skip work | `SymbolTableIsComplete` in release, log message construction | low | 2–5x |
| Avoid clones | Pass-by-`&mut`, dirty bits, custom Hash | medium | 2–4x |
| Better data structures | Persistent vectors, arena allocation, content-addressed cache | medium-high | 2–5x |
| Parallelism / incrementality | Independent-pass parallelism, incremental compilation | high | 3–10x |

Compounded, the four categories should produce a 10–100x speedup. The achievable target is a compile cycle that completes in <1 second for a typical widget — the same neighborhood as Rust's `cargo check` for a small library.

---

## 4 — Phased plan

Each phase ships independently and produces measurable speedup. Snapshot tests are the safety net at every step.

### 4.1 Phase 1: Skip work in release builds (~1 week, ~2-5x speedup)

The cheapest wins. No data-structure changes, no API changes.

**Deliverables:**
- **Gate `SymbolTableIsComplete` behind `#[cfg(debug_assertions)]`** or a feature flag. Run it once per stage in release builds; run it after every pass in development. The pass remains valuable; it just runs less often.
- **Replace `log::debug!()` in pass wrappers with lazy logging.** Use `log::log_enabled!(Level::Debug)` guards before constructing format strings, or move to `tracing` which has native lazy expansion.
- **Eliminate redundant `hash_value()` calls.** The current loop calls it twice per iteration (once at the top, once at the bottom). Cache the previous-iteration hash and only compute the post-pass hash.
- **Remove debug-only invariant checks from hot paths.** Identify other "this should never happen" checks in the pass code and gate them behind `debug_assertions`.

**Validation:** every existing test passes unchanged. Benchmark on a representative widget before and after; expect 2–5x improvement on `cargo build --release`.

### 4.2 Phase 2: Avoid clones (~3 weeks, ~2–4x additional speedup)

The structural change. Migrates the Pass trait from value-passing to mutable-reference-passing, adds dirty-bit tracking, and replaces hash-based fixed-point detection with explicit pass-reported changes.

**Deliverables:**
- **Pass-by-`&mut`.** Change every pass's signature:
  ```rust
  // Before
  pub trait Pass {
      fn run(input: Object) -> Result<Object, RHDLError>;
  }
  
  // After
  pub trait Pass {
      fn run(input: &mut Object) -> Result<bool, RHDLError>;  // returns `changed`
  }
  ```
  Mechanical refactor across all ~50 passes. Snapshot tests catch any pass that incorrectly reports unchanged when it actually changed.
- **Replace fixed-point `hash_value()` with summed `changed` flags.** The loop becomes:
  ```rust
  loop {
      let mut changed = false;
      changed |= RemoveUnneededMuxesPass::run(&mut obj)?;
      changed |= RemoveExtraRegistersPass::run(&mut obj)?;
      // ... etc ...
      if !changed { break; }
  }
  ```
  No hashing per iteration. Convergence detected in O(passes-per-iteration) instead of O(Object-size).
- **Custom Hash impl on Object.** When hashing is still needed (e.g., snapshot tests, content-addressed caching), skip irrelevant fields (source positions, debug spans, comments). The IR's *semantic* content hashes much faster than its full field-by-field representation.
- **Eliminate Object cloning in the iverilog test machinery.** Look at where Objects are cloned in the test infrastructure; many are unnecessary.

**Validation:** every existing test passes unchanged. Benchmark expected: 2-4x improvement on top of Phase 1. Combined Phase 1+2 target: 5–15x improvement.

### 4.3 Phase 3: Better data structures (~6 weeks, ~2–5x additional speedup)

The deeper structural change. Replaces the heap-allocated `Vec<...>` patterns with arena-allocated or persistent equivalents. Higher engineering risk because it touches the IR data shape, but well-bounded.

**Deliverables:**
- **Arena-allocated IR objects.** Replace `Vec<OpCode>` and similar with arena-allocated equivalents using `bumpalo` or `typed-arena`. The arena is owned by the Object; ops are referenced by `OpId(u32)`. Cloning an Object becomes a shallow operation (clone the arena's ID space, not its contents).
- **Persistent data structures for sparse updates.** Where passes do "modify a few entries in a large Vec," use `im::Vector` or `rpds::Vector`. Clone is O(1); update is O(log n); reads are O(log n) but with small constant factors. Not appropriate for tight inner loops, but appropriate for the high-level Object structure.
- **Slot-table persistent map.** The symbol table is the most-mutated part of the Object. Replace its `HashMap<Slot, ...>` (or whatever the current structure is) with a persistent immutable map. Most passes mutate ~1% of the entries; persistent maps amortize this beautifully.
- **Content-addressed pass cache.** When a pass receives an input it has seen before, return the cached output. This is the optimization-fuel idea from rustc — most passes are deterministic functions of their input. Cache hits skip the pass entirely.

**Validation:** snapshot tests catch any data-structure change that affects observable output. Benchmark target: another 2-5x on top of Phase 2.

### 4.4 Phase 4: Parallelism and incremental compilation (~8 weeks, ~3–10x additional speedup)

The largest individual effort. Two distinct sub-tracks.

**Sub-track 4a: Parallel passes.** Some passes operate on independent parts of the Object. `RemoveUnusedLiterals` and `RemoveUnusedRegisters` can run in parallel; they both walk the IR but neither writes the other's region. Identify a dependency graph between passes (which passes write what; which passes read what), and run independent passes concurrently using `rayon` or similar.

**Sub-track 4b: Incremental compilation.** Most kernel changes affect a small subset of the Object; the bulk of compilation work is reused. Implement an incremental cache that fingerprints inputs and reuses cached outputs. Conceptually similar to rustc's incremental cache, scoped to RHDL's IR.

**Deliverables:**
- **Pass-dependency graph.** A static analysis that identifies which passes can run in parallel based on their read/write sets over the Object. Each pass declares what fields it touches.
- **Parallel pass scheduler.** Replaces the sequential `wrap_pass<...>` calls with a scheduler that runs independent passes concurrently. Uses `rayon` for the worker pool.
- **Incremental compilation cache.** A persistent cache (on-disk via `target/rhdl-cache/`) that fingerprints kernel inputs and pass outputs. Cache lookups skip work entirely.
- **Pass-level `Profile`.** Per-pass timing data emitted as a `--timing-report` flag, so users can identify which passes dominate their compile time.

**Validation:** correctness via snapshot tests; performance via benchmark harness. Combined target: 30–100x improvement over Phase 0 baseline for a typical widget. Sub-second iteration loops become routine.

---

## 5 — Validation

Per CLAUDE.md §11.1, every phase is a compiler-level change with the full PR contract:

- **Snapshot tests are the safety net.** Every existing widget snapshot must remain byte-identical after each phase. The combined snapshot suite (~35 widgets, growing) is the regression detector.
- **Functional equivalence.** Every pre-change and post-change Object must produce byte-identical iverilog simulation output for the existing test inputs.
- **Benchmark harness.** A new `crates/rhdl-core/benches/compile.rs` benchmark suite that times `cargo build --release` of the FSM corpus and the AI/ML widgets (once they ship). Measured before each phase and after; results committed as a baseline for regression detection.
- **Per-PR perf budget.** A PR that *worsens* performance by more than 5% on the benchmark suite is blocked unless the slowdown is explicitly justified (e.g., a correctness fix that requires extra work).
- **Continuous benchmarking.** CI runs the benchmark on every PR and reports delta against `main`. The same machinery that catches snapshot regressions catches performance regressions.

---

## 6 — Risks and open questions

**Risk: pass refactor introduces subtle bugs.** Mechanical refactor of ~50 passes from value-passing to mutable-reference is a lot of code touch. Mitigation: snapshot tests are the safety net; refactor pass-by-pass with snapshot validation per pass; never bundle multiple pass refactors in one commit.

**Risk: persistent data structures have surprising performance characteristics.** `im::Vector` is O(log n) for most operations but with a 32-way trie internally, the constant factors are non-trivial. For very small Objects (which most kernels are), the persistent structures may be *slower* than the current `Vec`. Mitigation: benchmark before committing; have a fallback to `Vec` for small Objects via a configuration knob.

**Risk: incremental compilation is hard to get right.** rustc's incremental cache has had years of bugs. We'd be reinventing on a smaller scale, but the same hazards apply. Mitigation: ship incremental compilation as opt-in (`--incremental` flag) for the first release; gate it on extensive validation.

**Risk: parallelism introduces nondeterminism.** Pass A finishing before Pass B vs. after must not affect the output. Mitigation: the existing pass infrastructure is already designed for sequential execution; parallel execution requires the read/write set declarations to be precise. Snapshot tests catch any divergence.

**Risk: changes to Object shape break downstream consumers.** Surface-level changes (adding a `name_hint` field per `verilog-emission-plan.md` Phase 2; adding incremental-compile fingerprints) must not affect the public API of `rhdl-core`. Mitigation: keep public API stable; refactor internal representation only.

**Open question: where does the compile-time budget actually go?** This plan is informed by code reading but not by profiling. The first deliverable should be a profiling report (using `cargo flamegraph` or `samply`) on a representative widget. The phases above represent expected wins; profiling may reveal that the actual hot spots are elsewhere (e.g., proc-macro compile time, snapshot file I/O, monomorphization explosions).

**Open question: does Phase 3 (persistent data structures) actually help?** Most kernels are small enough that arena allocation may not pay off. The work is large; the benefit is unclear at small scales. Profiling required before committing.

**Open question: how does this interact with `rhif-formalization-plan.md`?** Adding source-name hints (per `verilog-emission-plan.md` Phase 2) and arena allocation (this plan's Phase 3) both touch the symbol table. The two plans should coordinate on the symbol-table redesign. Recommendation: ship Phase 1 (skip-work) and Phase 2 (pass-by-`&mut`) without changing the symbol table; reserve Phase 3 for after the verilog-emission Phase 2 ships and the symbol table has the new fields.

**Open question: monomorphization cost in `rhdl-fpga`.** Many widgets are generic over `T: Digital` and `const N: usize`. Each instantiation is a separate compilation unit. Generic widgets used at many type-and-N combinations may be paying significant monomorphization cost. Worth profiling.

---

## 7 — Sequencing recommendation

The recommended order:

**Step 0 (~1 week): Profile.** Build the benchmark harness; measure where compile time actually goes. Specifically: which passes dominate; what fraction is proc-macro vs. core-compile; where the `cargo check` vs. `cargo test` time differs; what the monomorphization cost looks like. The plan above is informed by code reading; profiling will shift priorities.

**Step 1 (Phase 1, ~1 week): Skip work in release.** Easiest wins. `SymbolTableIsComplete` in debug-only; lazy logging; cached hashes. Expected 2-5x.

**Step 2 (Phase 2, ~3 weeks): Pass-by-`&mut` and dirty bits.** Mechanical refactor; significant payoff; well-bounded risk.

**Step 3 (decision point): Profile again.** After Phases 1 and 2, where does the time still go? If the dominant cost has shifted, the Phase 3 / Phase 4 priority order may flip.

**Step 4a (Phase 3, ~6 weeks) and Step 4b (Phase 4, ~8 weeks): the deeper changes.** Run in parallel by different contributors if possible; both have well-isolated scopes.

Total elapsed time: ~4 months for the full plan, with the first measurable win available within 2 weeks.

---

## 8 — Comparison with other compiler-perf work

For grounding, what's the typical achievement on similar projects?

**rustc.** Spent many person-years optimizing query-based compilation, parallel codegen, and the incremental cache. Order-of-magnitude improvements over the past five years. Achievable scope; the techniques are well-documented.

**SWC** (Rust-based JS compiler). 50–100x faster than Babel via aggressive avoidance of allocations and parallelism. Demonstrates that a Rust-implemented compiler can be radically faster than incumbent ecosystems.

**Cranelift.** LLVM-equivalent in Rust, optimized for compile speed over output quality. 5–10x faster than LLVM at code generation. Demonstrates the value of compile-speed-as-feature.

**Yosys.** C++-based open-source synthesis. Single-threaded; the `synth` flow is the dominant compile cost in open FPGA flows. Contrast: a pass-parallel compiler at the IR level (RHDL) feeding into Yosys's single-threaded backend produces a hybrid where the front-end is fast but the back-end is the bottleneck.

**SpinalHDL / Chisel-FIRRTL.** Scala-based; incremental compilation via SBT amortizes the proc-macro-equivalent cost. The lesson: developer ergonomics improve dramatically when "compile" becomes "incremental compile."

The combined Phase 1+2 target (5–15x) is conservative against these benchmarks. The combined Phase 1+2+3+4 target (30–100x) puts RHDL in the same league as SWC for similar IR-traversal work.

---

## 9 — References

[1] Stoutchinin, A., Kostiuk, V., et al. *Efficient Compilation of Lazy Programs with Dependent Types.* — Persistent-data-structure techniques in compiler IRs. Useful reading for Phase 3.

[2] Klabnik, S., Nichols, C. *The Rust Programming Language*, Chapter on Smart Pointers. — Background on `Rc`, `Arc`, and the persistent-data-structure ecosystem in Rust.

[3] *bumpalo* crate (https://github.com/fitzgen/bumpalo). — Arena allocator for Rust. The recommended Phase 3 implementation.

[4] *im* and *rpds* crates. — Persistent immutable data structures for Rust. Alternative Phase 3 building blocks.

[5] *rayon* crate (https://github.com/rayon-rs/rayon). — Data-parallelism for Rust. The recommended Phase 4 work-stealing pool.

[6] rustc dev guide, *Query System*. — The rustc query-and-incremental-cache architecture. Reference for Phase 4b.

[7] SWC project documentation. — Practical case study of a 50–100x compile-speed improvement in a Rust compiler.

[8] *cargo-flamegraph* and *samply*. — The profiling tools recommended for Step 0.

[9] Lattner, C. *LLVM: A Compilation Framework for Lifelong Program Analysis & Transformation.* CGO 2004. — Background on multi-stage IR architectures and pass-management.

[10] *cranelift* code-generator documentation. — Compile-speed-optimized IR design as a counter-example to LLVM's optimization-quality-optimized design.

---

## 10 — Decisions captured

For the record (also reflected in `architecture.md` and `CLAUDE.md` once the plan ships):

- **Compile time is a strategic concern, not a development convenience.** The agent-driven workflow, the snapshot regression suite, and the competitive position against other HDLs all depend on tight feedback loops.
- **Profiling drives priorities.** This plan reflects code reading; the actual phase order may shift after Step 0 profiling. Don't commit to phases without measurement.
- **Snapshot tests are the safety net.** Every change ships behind the existing snapshot regression suite; functional behavior is preserved at every step.
- **Pass-by-`&mut` is the foundational change.** Phase 2 is the pivot point; everything after it depends on the pass infrastructure being mutable-reference-based.
- **Persistent data structures are a tactical tool, not a strategy.** Use them where they pay off (sparse updates to large structures); avoid them where they don't (small Objects, tight inner loops). Profile before committing.
- **Incremental compilation is opt-in for the first release.** Cache invalidation bugs are real; ship under a flag and stabilize before defaulting on.
- **Compile time is measurable.** A benchmark suite is part of the deliverable. Continuous benchmarking on PRs catches regressions.
- **The plan does not change observable behavior.** No emitted Verilog change; no IR semantic change; no public API change. Only execution speed differs.
