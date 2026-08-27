# Vendor-Primitive Architecture — Design Plan

A proposal for a target-provider trait system that lets RHDL widgets emit vendor-specific primitives (Xilinx DSP slices, Lattice EBR, Efinix multipliers, etc.) when targeting silicon that supports them, while gracefully falling back to portable Verilog on other targets. The architecture is single-source, single-IR, and target-agnostic at the widget-author level.

This is the third compiler-level design plan in the parallel set, alongside `auto-pipelining-plan.md` and `kernel-language-extensions.md`. All three are independently shippable; none of them depend on the others.

---

## 1 — Motivation

FPGA fabrics ship with hard primitives — multiplier-accumulators (DSP48E1/E2 on Xilinx 7-series, DSP58 on Versal, SB_MAC16 on Lattice iCE40), block RAMs (RAMB18/36, EBR, M9K), clock managers (MMCM, PLL_BASE, sb_pll40), differential I/O buffers (IBUFDS, OBUFDS), high-speed transceivers, distributed-RAM LUTRAMs, shift-register LUTs (SRL16E), tristate buffers, and more. Using them costs nothing at design time but gives drastically better area, timing, and power than synthesizing the equivalent function out of LUTs and flops.

Today RHDL has no first-class story for these. `rhdl-bsp` instantiates a few of them ad hoc — `ibufds.rs`, `open_collector.rs`, the OpalKelly XEM7010 board's `sys_clock.rs` — at BSP level, by name, through hand-written `vlog::ModuleDef` blocks. The widget library has no way for a widget like `Mac` (proposed in `widget-roadmap.md` Tier 2) to say "give me a 16×16 signed multiplier; use the best primitive available on the target."

The `widget-roadmap.md` MAC unit makes this concrete: on Xilinx 7-series a single DSP48E1 slice gives you a fully-pipelined 25×18 signed multiply-accumulate at hundreds of megahertz. On Lattice iCE40, no DSP slices — the same widget should fall back to a soft multiplier built from full-adders. A Verilog HLS team writes the widget twice (or once, with `if (XILINX) ...` guarded by macros). The Rust ecosystem can do better.

The goal is a *single* widget source whose Verilog output adapts to the target — without forking files, without Cargo features, without leaking target details into the widget's API.

---

## 2 — Design space

Four options were considered.

**Option A — Cargo features per target.** `cargo build --features xilinx-7series` flips `#[cfg(feature = "...")]` blocks. Familiar Rust pattern. Mutually-exclusive features are a Cargo anti-pattern; can't compile multiple targets in one workspace; doesn't scale beyond a small handful of targets; CI matrix becomes combinatorial. Rejected.

**Option B — Trait-bound target threading.** Each widget has a `T: Target` generic and uses associated types to select primitives. Type-safe but viral — every wrapping widget must thread the bound, and existing widgets become incompatible without a breaking-API rewrite. Quickly becomes Tcl-style "everything takes the interp." Rejected.

**Option C — Pattern-match in the codegen.** The IR carries only `Mul` (or whatever); the Verilog emitter looks at the target and pattern-matches "this 16×16 signed multiply fits a DSP48 profile." Brittle; the widget author can't be explicit about *which* primitive variant they want; multi-DSP patterns (pre-adder + multiplier + accumulator chains) are fragile to express. Useful as a complementary optimization but not as the main mechanism. Partially adopted.

**Option D — Late-bound target provider with default-impl trait.** The recommended architecture, detailed below.

---

## 3 — Recommended architecture: Target-provider trait with default-impl fallback

The architecture has four layers, each well-bounded.

### 3.1 IR layer — `PrimitiveRequest` opcode in NTL

NTL gains a structured "I need this kind of hardware" node:

```rust
pub struct PrimitiveRequest {
    pub kind: PrimitiveKind,
    pub params: PrimitiveParams,
    pub inputs: Vec<Vec<Wire>>,    // one Vec<Wire> per input port
    pub outputs: Vec<Vec<Wire>>,   // one Vec<Wire> per output port
    pub instance_name: Option<String>,
}

pub enum PrimitiveKind {
    SignedMul,
    UnsignedMul,
    SignedMac,
    DualPortBram,
    SinglePortBram,
    Lutram,
    Pll,
    Mmcm,
    Srl,
    Ibufds,
    Obufds,
    Tristate,
    DspChain,
    // ... extensible
}

pub struct PrimitiveParams {
    pub widths: SmallVec<[usize; 4]>,
    pub depths: SmallVec<[usize; 2]>,
    pub frequencies_hz: SmallVec<[u64; 4]>,
    pub options: BTreeMap<&'static str, ConstValue>,
    pub latency: Option<usize>,
}
```

This is conceptually an extension of the existing NTL `BlackBox` opcode — it carries an abstract description that says *what* hardware is needed without committing to any specific implementation. Auto-pipelining (per `auto-pipelining-plan.md`) treats `PrimitiveRequest` like `BlackBox`: an opaque latency-declared box that the pipeliner can place around but not inside.

### 3.2 Trait layer — `Target` with default impls per primitive kind

The Rustic core. Every method has a default that produces target-agnostic Verilog (or composes lower-level RHDL widgets). Specific targets override the methods for the primitives they have.

```rust
pub trait Target: Send + Sync + 'static {
    fn name(&self) -> &str;

    /// The device family this target represents, used to check that a
    /// design's named-primitive requirements can be met. See §3.6.
    fn family(&self) -> Family;

    /// Signed multiplier. Default: emit `assign p = $signed(a) * $signed(b);`.
    fn signed_mul(&self, p: &MulParams) -> TargetEmit {
        TargetEmit::generic_signed_mul(p)
    }

    /// Unsigned multiplier. Default: `assign p = a * b;`.
    fn unsigned_mul(&self, p: &MulParams) -> TargetEmit {
        TargetEmit::generic_unsigned_mul(p)
    }

    /// Multiply-accumulate. Default: instantiate the equivalent
    /// `signed_mul` followed by an adder.
    fn signed_mac(&self, p: &MacParams) -> TargetEmit {
        TargetEmit::generic_signed_mac(p, self)
    }

    /// Dual-port block RAM. Default: array of flops.
    fn dual_port_bram(&self, p: &BramParams) -> TargetEmit {
        TargetEmit::generic_flop_array_dpram(p)
    }

    /// Distributed RAM (LUTRAM-equivalent). Default: same as dual_port_bram.
    fn lutram(&self, p: &LutramParams) -> TargetEmit {
        self.dual_port_bram(&p.into())
    }

    /// Phase-locked loop / clock manager. Default: not supported.
    fn pll(&self, p: &PllParams) -> Result<TargetEmit, UnsupportedPrimitive> {
        Err(UnsupportedPrimitive::pll(self.name()))
    }

    /// Differential input buffer. Default: not supported.
    fn ibufds(&self, p: &DiffParams) -> Result<TargetEmit, UnsupportedPrimitive> {
        Err(UnsupportedPrimitive::ibufds(self.name()))
    }

    /// Tristate I/O buffer. Default: emit naked Verilog using `assign io = en ? out : 1'bz;`.
    fn tristate_io(&self, p: &TristateParams) -> TargetEmit {
        TargetEmit::generic_tristate(p)
    }

    /// Constraints this target needs alongside emitted Verilog.
    /// Default: none.
    fn constraints(&self, requests: &[PrimitiveRequest]) -> Vec<Constraint> {
        vec![]
    }

    /// Simulation models for iverilog round-trip testing.
    /// Default: none — primitives lower to standard Verilog and iverilog already understands them.
    fn sim_models(&self, requests: &[PrimitiveRequest]) -> Vec<SimModel> {
        vec![]
    }
}
```

`TargetEmit` is a small enum carrying either a `vlog::ModuleDef` (the actual Verilog), a `vlog::ItemInstance` (a hierarchical instance reference, for vendor primitives that already exist as library cells), or a `Delegate` marker that asks the framework to fall back to the default impl.

`UnsupportedPrimitive` is a structured error pointing at the primitive kind, the target name, and a suggestion (e.g., "use `cdc::handshake_bridge` for cross-domain transfer if no PLL is available").

### 3.3 Concrete targets

Targets are zero-sized types implementing `Target`:

```rust
pub struct GenericTarget;
pub struct Xilinx7Series;
pub struct XilinxUltraScalePlus;
pub struct LatticeICE40;
pub struct LatticeECP5;
pub struct EfinixTrion;
pub struct GowinGW1N;
pub struct MicrosemiPolarFire;

impl Target for GenericTarget {
    fn name(&self) -> &str { "generic" }
    // Accepts every default. PLL and ibufds remain Unsupported.
}

impl Target for Xilinx7Series {
    fn name(&self) -> &str { "xilinx-7series" }

    fn signed_mul(&self, p: &MulParams) -> TargetEmit {
        if p.fits_in_dsp48e1() { TargetEmit::dsp48e1_mul(p) }
        else { TargetEmit::Delegate }   // fall back to default
    }
    fn signed_mac(&self, p: &MacParams) -> TargetEmit {
        if p.fits_in_dsp48e1() { TargetEmit::dsp48e1_mac(p) }
        else { TargetEmit::Delegate }
    }
    fn dual_port_bram(&self, p: &BramParams) -> TargetEmit {
        if p.fits_in_ramb18(p) { TargetEmit::ramb18(p) }
        else if p.fits_in_ramb36(p) { TargetEmit::ramb36(p) }
        else { TargetEmit::Delegate }
    }
    fn pll(&self, p: &PllParams) -> Result<TargetEmit, _> {
        Ok(TargetEmit::mmcm_adv(p))
    }
    fn ibufds(&self, p: &DiffParams) -> Result<TargetEmit, _> {
        Ok(TargetEmit::ibufds_xilinx(p))
    }
    // ... overrides for tristate buffers, OBUFDS, etc.
}
```

The default-impl pattern means a partial `Target` implementation is always valid. You can ship a `Xilinx7Series` provider that only overrides `signed_mul` and `dual_port_bram`; everything else falls through to `GenericTarget`-equivalent emission. The hierarchy is implicit — there is no "generic ← xilinx" trait relationship; `GenericTarget` is just the trait's default-impl behavior expressed as a concrete type.

### 3.4 Compile-pipeline integration

The compile pipeline gains a target argument *only at the Verilog-emission step*. The IR pipeline (Rust → RHIF → RTL → NTL) is target-agnostic.

```rust
let mac: Mac<b16, b16, 32> = Mac::default();

// Target-agnostic NTL — same for every target.
let descriptor = mac.descriptor("mac".into())?;

// Target-specific Verilog emission.
let hdl_xil = descriptor.hdl_for(&Xilinx7Series)?;       // DSP48-flavored
let hdl_lat = descriptor.hdl_for(&LatticeICE40)?;        // SB_MAC16-flavored
let hdl_gen = descriptor.hdl_for(&GenericTarget)?;       // pure portable Verilog

// iverilog round-trip uses GenericTarget by default for portability.
test_bench.rtl(&mac, &Default::default())?.run_iverilog()?;
```

The new `hdl_for` method on `Descriptor` walks the NTL, emits standard ops as Verilog directly, and dispatches `PrimitiveRequest` ops through the `Target` trait. Constraints are collected as a side-output and returned in a structured `HDLDescriptor` that the BSP can write to `.xdc` / `.pcf` / `.sdc`.

### 3.5 Two ways for widget authors to request a primitive

**Pattern A — Pure RHDL, compiler-driven recognition.** Write the kernel using normal arithmetic. The NTL→Verilog emitter pattern-matches on the structure: a 16×16 multiply with operand-and-result widths fitting a DSP48 profile gets emitted as a `PrimitiveRequest(SignedMul, ...)` automatically. This is Option C from §2, used as a *complementary* optimization within the recommended architecture. Zero user-facing change. Conservative: applied only when the match is unambiguous.

**Pattern B — Explicit primitive request via macro.** When the user knows they want a specific primitive — say, a true-dual-port BRAM with very specific port widths and a write-first read-during-write policy, or a transceiver with specific link configuration — use a macro:

```rust
let prod = primitive!(SignedMul {
    a: i.a,
    b: i.b,
    accumulator: q.acc,
    latency: 3,
});
```

The macro expands into a `PrimitiveRequest` op in the IR with explicit parameters. The widget author has chosen "this is a primitive boundary"; the target picks the implementation; the default impl does the right thing if no override exists. This is the escape hatch when pattern recognition isn't enough.

For the MAC widget, both patterns work. Pattern A is the natural starting point. Pattern B is for the rare case where the user wants *exactly* the DSP48E1 pre-adder + multiplier + accumulator pipeline pattern with a specific latency, where pure-Rust arithmetic would not unambiguously map to that shape.

---

### 3.6 What this trait deliberately does not cover: primitives asked for by name

Every method above is a *capability* — a multiplier, a block RAM, a PLL. The widget says what it wants done and the target says how; a target that has no opinion inherits a default. That is the right shape for capabilities and it is the shape this whole document is about.

It is the wrong shape for a primitive requested **by name**. A widget wrapping `MUXF7` or `ISERDESE2` or `PS7` is not asking for a capability — it wants that block, for its timing or its placement or its cascade port — and there is no abstraction to hide behind. Encoding those as trait methods would mean one method per primitive, and the Xilinx 7-series library alone has around two hundred. `Target` does not scale there and should not try.

Named primitives are handled instead as *data*: a black-box declaration (`rhdl-core/src/circuit/blackbox_decl.rs`) names the module and the device families that provide it, and the requirement is discovered by walking the constructed widget tree rather than declared in a type signature. **The design is specified in `xilinx-primitive-library.md` §7**, which covers the `Requirement` type, where it is checked, the diagnostics, and how a widget offers different named primitives on different targets without the tree diverging from the Rust simulation.

Two things in that design bear on this document directly:

- It needs only `Target::name()` and `Target::family()` — a new method returning the device family the target represents. None of `PrimitiveRequest`, no NTL change, none of §3.2's capability surface. So it can land on a two-method trait skeleton well before Phase 1 has substance, and Phase 1 then fills the same trait in.
- It makes `hdl_for(&target)` the enforcement point for *both* kinds of divergence: `UnsupportedPrimitive` for a capability the target lacks, and a portability error for a named primitive the target cannot provide. The two failures are different in origin and should stay different in the diagnostic, but they arrive at the same call, which is the property that matters.

---

## 4 — Why this is the Rustic answer

It mirrors the pattern Rust already uses for `Default`, `Iterator`, `Hasher`, `serde::Serializer` — a trait with default impls where most methods rarely need overriding. Adding a new primitive kind is one method on the trait. Adding a new target is one impl block. Adding a widget that wants a primitive doesn't change the trait or any target.

The default-impl fallback means that for every primitive *this trait covers*, you can always compile to any target. At worst you pay area/timing cost for a missing primitive. No mutually-exclusive features. No source-level forking. The IR stays the single source of truth.

That guarantee does not extend to primitives asked for by name (§3.6), and it cannot: nothing portable stands in for `PS7`. What replaces it there is not a fallback but an *error you get from RHDL* — a design that names Xilinx silicon fails at `hdl_for(&LatticeICE40)` with the instance path, rather than succeeding and failing later in someone else's toolchain. Different guarantee, same discipline: the constraint is stated where the compiler can check it.

The `hdl_for(&target)` method on `Descriptor` is itself a Rust-idiomatic dispatcher — it doesn't move or restructure the design; it just chooses how to print it.

---

## 5 — Phased rollout

### Phase 1 — Architecture + two primitives (4–6 weeks)

- Add `PrimitiveRequest` opcode to NTL (`crates/rhdl-core/src/ntl/spec.rs`).
- Define the `Target` trait, `TargetEmit`, `UnsupportedPrimitive`, and the parameter structs.
- Implement `GenericTarget` (all defaults) and `Xilinx7Series` (with overrides for `signed_mul`/`signed_mac` mapping to DSP48E1, and `dual_port_bram` mapping to RAMB18).
- Add `hdl_for(&Target)` to `Descriptor`.
- Rewrite `core::ram::synchronous::SyncBRAM` to emit a `DualPortBram` request rather than the existing flop array. Verify (a) `GenericTarget` produces today's Verilog byte-for-byte and (b) `Xilinx7Series` produces a `RAMB18` instantiation. Both pass `iverilog` round-trip with appropriate sim models.
- Document the architecture in the RHDL book under a new chapter `targets/`.

This is the validation-of-concept milestone. One widget, two targets, end-to-end verified.

### Phase 2 — DSP-MAC and the first MAC widget (4–6 weeks)

- Add `SignedMac` primitive kind with parameters covering pre-adder, multiplier, accumulator, and latency.
- Override `signed_mac` on `Xilinx7Series` to emit DSP48E1 in MAC mode.
- Build the `dsp::mac` widget from `widget-roadmap.md` Tier 2 (#15) using `primitive!(SignedMac { ... })`.
- Verify: same widget, three targets (`GenericTarget`, `Xilinx7Series`, `LatticeICE40` once added), all functionally equivalent.

### Phase 3 — Lattice ECP5 and Efinix targets (~6 weeks)

- Add `LatticeECP5` (`MULT18X18`, `DP16KD`, `EHXPLLL`).
- Add `EfinixTrion`.
- Rewrite or instantiate the BSP-level Lattice and Efinix support to use the new target system. Existing `rhdl-bsp/src/drivers/xilinx/` and `rhdl-bsp/src/drivers/lattice/` move into target overrides.

### Phase 4 — PLL/MMCM and clock-domain-aware primitives (~6 weeks)

- Add `Pll` and `Mmcm` primitive kinds with frequency parameters and lock-output ports.
- Targets that can synthesize specific output frequencies override; targets that cannot return `Err(UnsupportedPrimitive::Pll)` and the user gets a clear compile-time error.
- Constraint emission for PLLs (clock-period XDC) flows out of `target.constraints(...)`.

### Phase 5 — Tristate / IBUFDS / OBUFDS / SRL (~3 weeks)

- Round out the I/O primitive set. Mostly trivial overrides per target.

### Phase 6+ — High-speed transceivers, encrypted IP wrappers, vendor-specific compute blocks (open-ended)

- GTH/GTX, GTY, MGT primitives on Xilinx.
- Hardened SerDes, PCIe blocks.
- These typically have their own configuration GUIs (Vivado IP Integrator, Lattice Diamond IPexpress); RHDL's role is to instantiate them with parameters and let the user supply the IP-config artifact.

---

## 6 — Validation

Per `CLAUDE.md`'s contract, every primitive that ships in this system must have:

**Tier 1 — Trait-method unit test.** Construct a `PrimitiveRequest`, call the target's method, verify the emitted `TargetEmit` matches an `expect_test` snapshot of the expected Verilog.

**Tier 2 — Target-matrix simulation.** A widget that uses the primitive simulates identically across all supported targets in the iterator-based simulator. The Rust simulator does not know about primitives directly; it sees the design through the same iterator pipeline. Functional equivalence at the simulation layer is the strongest cross-target invariant.

**Tier 3 — `iverilog` round-trip with vendor sim models.** When a target has overrides that emit vendor primitives, the `Target::sim_models` method must supply iverilog-compatible behavioral models (or stubs) so the round-trip test passes. For Xilinx 7-series, `unisims`-equivalent stubs ship with the BSP.

**Tier 4 — Constraint-emission test.** When a primitive needs a constraint (e.g., MMCM clock-period XDC), the constraint string is captured in an `expect_test` snapshot.

**Tier 5 — Real-hardware smoke test (optional, BSP-gated).** For a small set of canonical widgets, document the Vivado / nextpnr-ice40 / nextpnr-ecp5 / Efinity build flows and a "blink-LED-from-MAC-output" demo. This is the proof-of-life evidence that the emitted Verilog actually works on silicon. Out of scope for the trait infrastructure itself, but a milestone for the BSPs.

A *meta-test* in `rhdl-fpga` constructs a synthetic design that uses every primitive kind, compiles it under each registered target, and asserts: (a) the Rust simulator produces identical waveforms; (b) every target's Verilog passes `iverilog` round-trip; (c) constraint sets are non-overlapping and well-formed.

---

## 7 — Interaction with the other parallel design tracks

**Auto-pipelining.** A `PrimitiveRequest` carries a declared latency. The auto-pipeliner treats it as a fixed-latency black box: registers can be inserted before and after, but never inside. This is the same way the existing `BlackBox` opcode behaves. No conflict; no new design choices needed.

**Kernel-language extensions.** The `primitive!` macro is a new piece of macro-layer code. It composes naturally with `kernel-language-extensions.md` Phase 5 (closure desugaring), since both transform user-level syntax into IR-level constructs. They can ship in either order.

**Widget roadmap.** Several Tier-1 and Tier-2 widgets in `widget-roadmap.md` will benefit from primitives:
- MAC unit (#15) → DSP-MAC primitive on Xilinx, soft-MAC on others.
- Generic memory-mapped register file (#17) → uses LUTRAM on targets that have it.
- Integer divider (#14) → could use DSP slice on Xilinx (multiplicative inverse), pure logic elsewhere.
- Multi-bit handshake bridge (#4) → could use distributed-RAM toggling on targets with LUTRAM.

The roadmap can be re-prioritized once Phase 1 of this architecture is complete: widgets that depend on vendor primitives become both higher-value (target-optimal) and lower-risk (compiler does the work).

---

## 8 — Risks and open questions

**Versioning.** DSP48E1 vs. DSP48E2 vs. DSP58 — same conceptual primitive, different parameters and footprint. The `Xilinx7Series` target picks DSP48E1; a hypothetical `XilinxVersal` target picks DSP58. But mid-life part variants within a family (e.g., Artix-7 vs. Kintex-7) share the primitive but have different available counts and locations. The right granularity for `Target` is *primitive availability*, not *part number* — concrete part details belong in the BSP.

**Heterogeneous targets.** A single design might want some IP from Xilinx (BRAM for one block, soft logic for another). The `Target` trait is per-design at the moment; allowing per-instance target selection (e.g., `#[rhdl(target_override = "soft")]` on a sub-circuit) is a possible extension but is not in the Phase 1 scope.

**Constraint propagation across hierarchy.** Each `PrimitiveRequest` has an `instance_name`. Constraints typically reference a hierarchical path (`top/mac0/dsp_inst`). The compile pipeline must thread the hierarchical scope into the constraint emission. The existing `ScopedName` machinery in `rhdl-core` is the right home for this.

**Simulation-model fidelity.** Xilinx's `unisims` library is hundreds of thousands of lines of behavioral Verilog; we do not need to reproduce all of it. We need just enough to make iverilog round-trip pass for the primitives we lower to. A practical first pass is hand-written stubs for DSP48E1 (multiply-accumulate) and RAMB18 (dual-port) that pass our test corpus, with a clearly-documented "this is not a complete vendor-equivalent model" caveat.

**Encrypted IP and proprietary primitives.** Some vendor primitives (high-speed transceivers, hardened CPU blocks) are encrypted or only callable through vendor-IP wizards. RHDL cannot synthesize Verilog for these; it can only instantiate them by name and rely on the vendor toolchain for elaboration. The architecture supports this (the `TargetEmit` variant for "instance reference to existing IP") but the BSP must supply the IP.

**Pattern A (compiler recognition) versus Pattern B (explicit `primitive!`) ergonomics.** Phase 1 ships Pattern B only. Pattern A — the implicit recognition of multiply patterns to DSP requests — is a Phase 2+ optimization once we have empirical data on what patterns users actually write. Otherwise the recognition rules ossify around current code rather than around what users want.

**LLM-driven target selection.** Most users will want the compiler to pick a sensible default — "use whatever the connected BSP provides." An AI agent generating a widget should not need to know about targets at all. The architecture supports this via `compile_design_for_default_target::<K>()` where the default is `GenericTarget` plus whatever the BSP injects. Worth specifying clearly in the user-facing API.

---

## 9 — References

[1] Basu, Samit. "RHDL: Rust as a Hardware Description Language." LATTE '25, March 2025. (`doc/latte25/latte.tex`.)

[2] Xilinx, Inc. *7 Series FPGAs DSP48E1 Slice User Guide* (UG479). https://docs.xilinx.com/v/u/en-US/ug479_7Series_DSP48E1 . Definitive description of the DSP48E1 primitive RHDL targets first.

[3] Xilinx, Inc. *7 Series FPGAs Memory Resources User Guide* (UG473). https://docs.xilinx.com/v/u/en-US/ug473_7Series_Memory_Resources . RAMB18/RAMB36 documentation.

[4] Xilinx, Inc. *7 Series FPGAs Clocking Resources User Guide* (UG472). https://docs.xilinx.com/v/u/en-US/ug472_7Series_Clocking . MMCM_ADV / PLL_BASE.

[5] Lattice Semiconductor. *iCE40 LP/HX Family Data Sheet* (FPGA-DS-02029). https://www.latticesemi.com/iCE40 . SB_MAC16, EBR, sb_pll40 primitives.

[6] Lattice Semiconductor. *ECP5 and ECP5-5G Family Data Sheet* (FPGA-DS-02012). https://www.latticesemi.com/ECP5 . MULT18X18D, DP16KD, EHXPLLL.

[7] Efinix, Inc. *Trion FPGA Architecture and Resources*. https://www.efinixinc.com/products-trion.html .

[8] CIRCT Project. "Circuit IR Compilers and Tools." https://circt.llvm.org/ . The `firrtl-to-hw` lowering and CIRCT's vendor-aware passes are the closest open-source precedent for what this architecture proposes; CIRCT separates target-agnostic IR from per-target lowering at the MLIR pass level. The `Target` trait here is a Rust-native equivalent.

[9] LLVM Project. "Target Description Files." https://llvm.org/docs/WritingAnLLVMBackend.html . LLVM's `TargetMachine` / `TargetLowering` pattern is the canonical example of late-bound target-specific lowering on a target-agnostic IR. The RHDL design here adopts the high-level pattern (target-agnostic IR + target-specific lowering trait) without LLVM's specific abstractions.

[10] Bachrach, J., et al. "Chisel: Constructing Hardware in a Scala Embedded Language." DAC 2012. — Chisel uses CIRCT for backend lowering; its target-specific concerns are addressed in FIRRTL transforms rather than in the user-facing language.

[11] Skarman, F., Gustafsson, O. "Spade: An Expression-Based HDL With Pipelines." OSDA 2023. — Spade today does not have a vendor-primitive system; this design plan is partially informed by what's missing in Spade, as a comparison point.

---

## 10 — Decisions captured

For the record (also reflected in `CLAUDE.md`):

- **Single source, single IR.** Widgets are written once. The IR carries `PrimitiveRequest` opcodes; the per-target divergence happens only at Verilog emission.
- **Target as a parameter to `hdl_for(...)`, not as a generic on widgets.** Widgets remain target-agnostic in their type signatures; the target is supplied at the codegen call.
- **Default-impl trait fallback.** Every target inherits a portable-Verilog default for every primitive kind via the `Target` trait's default methods. No target needs to override every method.
- **Two ways to request a primitive from a widget.** Implicit (compiler pattern recognition, deferred to Phase 2+) and explicit (`primitive!` macro, shipped in Phase 1).
- **Cargo features are not the right tool for target selection.** Target choice is a runtime/codegen-time argument, not a compile-time feature flag.
- **Capabilities go through the trait; named primitives go through data.** `Target` methods cover primitives RHDL can abstract (multiply, BRAM, PLL). A primitive requested by name is declared in a black-box library and carries the device families that provide it; the requirement is computed by walking the widget tree and checked at `hdl_for`. See §3.6 and `xilinx-primitive-library.md` §7.
- **Target-dependent choice of a named primitive happens at construction, at a black-box leaf.** Not at emission: the Rust simulation has no target, so a widget holding both alternatives would model one branch while emitting the other. A leaf built for a target, with the portability check as the interlock, keeps descriptor, netlist and simulation describing the same silicon.
- **Constraints flow with primitives.** A primitive's instantiation produces both Verilog and any required constraint fragments, both scoped to the instance's hierarchical name.
- **BSPs supply concrete part details and simulation models.** The `Target` trait describes primitive availability; the BSP layer (`rhdl-bsp`) supplies part-specific details, pin-out constraints, and iverilog-compatible simulation stubs.
