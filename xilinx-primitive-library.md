# The Xilinx Primitive Library

> **Status: an execution plan, to be carried out on a machine with Vivado installed.** This document specifies how to build a complete black-box declaration library for the Xilinx 7-series — the primitives a Zynq-7020 provides — how to *verify* the connectivity claims by simulation rather than merely asserting them, what has to exist in `rhdl-core` first, and how a design that uses any of it declares the silicon it needs (§7) so that building it for the wrong device is an error rather than a surprise from someone else's toolchain.
>
> It is written to be executed by someone (or something) sitting at a machine with `$XILINX_VIVADO` populated. Every step that needs the vendor tools is marked **[VIVADO]**. Everything else can be done anywhere.
>
> Prerequisites: `black-box-connectivity.md` (all three phases shipped). This is the Phase 4 that document defers.

---

## 1 — Why this cannot be written from memory

The declaration format exists and works. What is missing is two hundred entries of accurate data, and the accuracy matters in an unusual way.

**A connectivity declaration is believed.** `ntl::graph::make_net_graph` adds exactly the edges a black box declares; the composition-level cycle detector treats them as fact. So a primitive declared `None` that actually feeds through is a combinational path the compiler asserts does not exist — the precise bug Phase 1 removed, reintroduced as data, and worse than the original because it arrives in a checked-in file with a `source:` field citing UG953.

Port lists are the smaller problem and still a real one. The simple primitives are memorable — `IBUF`, `FDRE`, `LUT6`, `MUXF7`, `CARRY4`, `ODDR`. The ones that matter for real designs are not: `DSP48E1` has around fifty ports including cascade paths, `RAMB36E1` around a hundred, `ISERDESE2` and `OSERDESE2` similar, and `PS7` — the block that makes a Zynq a Zynq — has several hundred, being every AXI interface the processing system exposes.

So the library must be *extracted*, not recalled. Fortunately almost all of it can be, and the part that cannot be extracted can be **tested**.

---

## 2 — What has to exist first: referencing a module RHDL did not write

**Today this is impossible**, and it blocks everything downstream. `Descriptor::hdl()` calls `ModuleList::checked()`, which runs `iverilog -t null` over the emitted text. Icarus rejects an instantiation it cannot resolve:

```
top.v:2: error: Unknown module type: MUXF7
```

The descriptor cannot be built, so the widget cannot exist. This is why every black box in the tree *defines* its Verilog rather than instantiating someone else's: `core::dff` writes an `always` block, it does not instantiate `FDRE`.

### 2.1 — The fix, which has been prototyped

Supply Icarus with port-only stub definitions, generated from the declarations the library already carries, written alongside the design **for the check only** and never emitted.

In `rhdl-vlog`:

```rust
impl ModuleList {
    /// Check the module list for syntactic correctness using Icarus Verilog.
    pub fn checked(&self) -> anyhow::Result<()> {
        self.checked_with_stubs(&[])
    }

    /// Check the module list, supplying stand-in definitions for modules
    /// defined outside this design.
    ///
    /// `stubs` are port-only module definitions, written alongside the
    /// design for the check only.  They are never emitted: at synthesis
    /// the vendor's real library supplies the definition, and a stub in
    /// the output would collide with it.
    ///
    /// Passing an empty slice is the old behaviour exactly, so an
    /// undeclared module is still an error.  This widens what can be
    /// described; it does not weaken the check.
    pub fn checked_with_stubs(&self, stubs: &[ModuleDef]) -> anyhow::Result<()> {
        let d = tempfile::tempdir()?;
        let d_path = d.path();
        std::fs::write(d_path.join("top.v"), self.to_string())?;
        let mut cmd = std::process::Command::new("iverilog");
        cmd.arg("-t").arg("null").arg(d_path.join("top.v"));
        if !stubs.is_empty() {
            let text = ModuleList { modules: stubs.to_vec() }.to_string();
            std::fs::write(d_path.join("stubs.v"), text)?;
            cmd.arg(d_path.join("stubs.v"));
        }
        // ...unchanged from here
    }
}
```

That much compiles and is the whole of the `rhdl-vlog` change. It was prototyped and then reverted, so this document could be the deliverable; reinstating it is a copy-paste.

### 2.2 — Threading the declarations through

`HDLDescriptor` gains the modules it references:

```rust
pub struct HDLDescriptor {
    pub name: String,
    pub modules: rhdl_vlog::ModuleList,
    /// Modules this design instantiates but does not define.
    ///
    /// Stubbed for `checked()`, and named to the toolchain so the vendor
    /// library is linked at synthesis and simulation.
    pub externals: Vec<ExternalModule>,
}

pub struct ExternalModule {
    pub name: String,
    pub ports: Vec<rhdl_vlog::Port>,
}

impl ExternalModule {
    /// From a library declaration, which already has exactly this.
    pub fn from_decl(decl: &BlackBoxDecl) -> Self { /* ... */ }
    /// A port-only `ModuleDef`, for the check.
    pub fn stub(&self) -> rhdl_vlog::ModuleDef { /* ... */ }
}
```

`Descriptor::hdl()` changes from `hdl.modules.checked()?` to a call that passes the stubs. The one existing call site is `crates/rhdl-core/src/circuit/descriptor.rs:75`.

**Design note worth preserving:** stubs are generated from the *declared* ports, so the declaration is load-bearing twice over — it feeds the reachability analysis and it feeds the syntax check. A wrong width in the library becomes a stub that does not match the instantiation, and Icarus says so. That is a second, independent check on the library's port data, for free.

### 2.3 — Externals must propagate up the hierarchy

A widget instantiating `MUXF7` may be a child of a widget that knows nothing about it. `build_synchronous_hdl` and `build_circuit_hdl` collect children's HDL; they must also union children's `externals`, or the top-level check will fail on a module a child declared. Straightforward, and easy to forget.

### 2.4 — What it must not become

A blanket "skip the check". An **undeclared** module must still be an error, because that is a typo in an instantiation and the current behaviour catches it. The test for this phase is a widget instantiating a module it did not declare, and asserting it still fails.

---

## 3 — Extracting the port lists **[VIVADO]**

Mechanical, and should be scripted rather than typed.

### 3.1 — Where the sources are

```sh
# One file per primitive, module name == file name.
ls "$XILINX_VIVADO/data/verilog/src/unisims/"        # simulation primitives
ls "$XILINX_VIVADO/data/verilog/src/retarget/"       # macros that map to primitives
ls "$XILINX_VIVADO/data/verilog/src/unifast/"        # fast, less accurate models
```

`unisims` is the set to use: it is the behavioural model Vivado itself simulates against, so its port list is definitionally correct and its body reveals the connectivity.

To narrow to what a Zynq-7020 actually has, rather than all of 7-series:

```sh
# The part's primitive list, straight from the tool.
vivado -mode batch -nolog -nojournal -source - <<'TCL'
link_design -part xc7z020clg400-1
foreach cell [lsort [get_lib_cells]] { puts [get_property NAME $cell] }
TCL
```

Take the intersection with `unisims/*.v`. Expect roughly 200 primitives; the difference between "7-series" and "Zynq-7020" is mostly transceiver and higher-speed-grade parts, plus `PS7` existing only on Zynq.

### 3.2 — The extraction script

A small program that, for each `unisims/NAME.v`:

1. Parses the module header for ports: name, direction, width.
2. Classifies each port's role: `Clock` if it appears in a `posedge`/`negedge` event expression, `Reset` if its name matches the vendor's conventions (`R`, `S`, `RST`, `CLR`, `PRE`, `GSR`) *and* it appears in a reset branch, else `Data`.
3. Proposes connectivity by classifying the body (§4).
4. Emits a RON entry, with `source:` recording the Vivado version.

Two implementation notes:

- **Do not use `rhdl-vlog`'s parser for this.** It implements a subset of Verilog aimed at what RHDL emits; `unisims` models use `specify` blocks, `defparam`, `$recovery` timing checks, `UNISIM` compiler directives and much else. Extracting a module header is a small enough job for a purpose-built scanner, and fighting the AST parser into accepting vendor sources is a separate and larger project. (Doing that anyway would let the cross-check test in `crates/rhdl-bsp/tests/primitive_library.rs` point at real sources, which is a genuine follow-up — but not a prerequisite.)
- **The script lives outside the build.** `tools/extract-unisim-connectivity/`, run by hand when a Vivado version changes, with its output checked in. It must not run during `cargo build`: the library is data under review, not a build artefact, and nobody should be able to change what the compiler believes about silicon by having a different Vivado installed.

---

## 4 — Connectivity: the judgement, and how to check it

The part that cannot be extracted mechanically with confidence. But it can be *tested*, which is better than either extracting or asserting it.

### 4.1 — Reading it from the model

The `unisims` body tells you, if you know what to look for:

| pattern | means |
|---|---|
| `assign O = f(I0, I1);` | combinational: every read reaches `O` |
| `always @(*)` / `always @(A or B)` | combinational |
| `always @(posedge CLK)` | registered: no path from the reads to the writes |
| `always @(posedge CLK or negedge RST)` | registered, async reset — still no data path |
| `if (ATTR == "TRUE") ... else ...` around either | **attribute-dependent**, see §4.3 |

So a first pass can classify most primitives automatically: a model with no `posedge` anywhere is combinational; one where every output is written only inside a `posedge` block registers everything.

### 4.2 — Testing it, which is the part worth doing properly **[VIVADO]**

With Vivado installed, connectivity stops being a judgement and becomes an experiment. Build a harness that, for each primitive:

1. Instantiates the real `unisims` model.
2. Holds every clock **static** — never toggled for the whole run.
3. For each data input in turn, toggles it and records whether any output changes.
4. An output that changes with no clock edge anywhere **proves** a combinational path from that input.

```sh
# Per primitive, roughly:
xvlog -sv harness_MUXF7.sv
xvlog "$XILINX_VIVADO/data/verilog/src/unisims/MUXF7.v"
xelab -debug typical harness_MUXF7 -s sim_MUXF7
xsim sim_MUXF7 -runall
```

**The asymmetry is the point, and it is the right way round.** Observing a change proves a path exists, so this can **refute** a `None` declaration — which is the unsound direction, the one that hides a real loop from the compiler. Not observing a change proves nothing: the path might need a different attribute setting, or a state the harness never reached. So:

- Harness says "path exists", library says `None` → **the library is wrong.** Hard failure.
- Harness says "no path observed", library says `Paths(...)` → the library is conservative. Fine; note it.
- Harness says "no path observed", library says `None` → consistent, though unproven.

Encode exactly that in the harness's exit condition. A test that can only fail in the direction that matters is more useful than one that tries to be exhaustive and ends up asserting nothing.

### 4.3 — Attribute-dependent primitives

`DSP48E1` with `AREG`/`BREG`/`CREG`/`PREG`, block RAM with `DO_REG`, `ISERDESE2` in its several modes: whether a path is combinational depends on how the primitive is configured.

**Declare the conservative case** — the configuration with the *most* combinational paths, usually all pipeline registers set to zero — and record the dependence in the entry's `note`, which surfaces in diagnostics. A user who has set `PREG=1` gets a conservative answer, which costs them a spurious feedthrough report; a user who has set `PREG=0` gets a correct one. The reverse choice would hide a real path from the second user, which is not a trade worth making.

The harness should sweep the attribute space for these, so the note can say which configurations were actually tested rather than which were assumed. Keep the sweep small and named: `DSP48E1` alone has enough attributes to make an exhaustive sweep pointless.

### 4.4 — `PS7`

The Zynq processing system. Several hundred ports, almost all AXI, and its `unisims` model is largely a shell.

Declare it `Opaque` and move on. Every AXI interface is registered in practice, so `Opaque` is conservative rather than wrong, and the alternative — auditing several hundred ports to establish which of them could feed through — is a great deal of work to turn a conservative answer into a slightly less conservative one. The note should say this explicitly so the next reader does not redo the analysis to discover it was not worth doing.

---

## 5 — Drivers, and the connectivity they owe

A `Driver` is not a circuit: no descriptor, no matrix, assembled into the fixture after the widget tree, and therefore invisible to every analysis. `crates/rhdl-bsp/src/ok/drivers/xem7010/host.rs` mounts **eleven** circuit inputs and **seven** circuit outputs through the external `okHost` module, and nothing checks any of it.

The ask is that drivers declare their connectivity even when it is nothing. That needs the fixture to have a reachability matrix, which is a real piece of work and mostly independent of the library:

1. `Driver` gains a `connectivity: BlackBoxConnectivity` — or, better, its own declaration referencing a library entry, since `IBUFDS` should not have to say twice that it is a buffer.
2. The fixture composes drivers' matrices with the wrapped circuit's, over the mount points, and runs the same cycle check.
3. A driver mounting both an input and an output becomes an edge from `O` back to `I`, which is exactly the shape that can close a loop outside the circuit tree.

Note the answers are **not** all null, and the ask's phrasing invites a mistake here. `IBUFDS` is a buffer: its honest declaration is `Paths([("I","O"), ("IB","O")])`. `OBUF` likewise. The null ones are the registered ones — `IDDR`, `ODDR`, an `MMCME2` output, the OpalKelly host's AXI-like interfaces. Getting `IBUFDS` wrong as `None` would be exactly the unsound direction, on the most-used primitive in the crate.

**Sequencing:** this can be done before or after the library, but it should be done before the examples, because a realistic example instantiates an I/O buffer and it would be odd to ship one whose boundary is unanalysed.

---

## 6 — Examples

Three, in increasing order of what they demonstrate. All need §2; the third needs §5.

1. **A primitive in a circuit.** `MuxF7` already exists but emits a behavioural equivalent. With §2 it emits a real `MUXF7` instantiation. The example shows the widget, the emitted Verilog with the instantiation in it, and — the point — the DRC reporting a combinational path through it. Trace committed as usual.
2. **A primitive that changes an analysis result.** Two `MUXF7`s wired into a ring: a combinational loop reported at widget level, naming both. Then the same design with an `FDRE` on one edge, building cleanly. This is the demonstration that the declarations are load-bearing rather than decorative, and it is the example most worth having, because the failure it shows was invisible before this work.
3. **A primitive at the boundary.** An `IBUFDS` differential clock input feeding a counter, with the constraint file emitted alongside. Shows the driver path, the constraints, and — with §5 — the boundary participating in the analysis.

**[VIVADO]** Each example gains something the tree cannot currently have: a real simulation. `xsim` against `unisims` can run these end to end, which iverilog cannot, so a primitive-wrapping widget can finally have a Tier-4 equivalent. Worth wiring as an opt-in test that skips when `$XILINX_VIVADO` is unset, in the same spirit as the `iverilog_precondition` test — present, honest about being skipped, and not silently absent.

---

## 7 — Device-specific code, and circuits that span devices

This library manufactures a problem the tree does not have today. Two hundred widgets, each of which wraps a module that exists only in Xilinx silicon, and nothing anywhere records that fact. A design containing a `MuxF7` cannot be synthesised for an ECP5, and the way you would find that out is a Lattice tool complaining about an unknown module — after RHDL had reported success.

So the library needs a companion mechanism: a way for a widget to say which silicon it needs, a place that checks it, and a way for a widget to offer *different* primitives on different targets. `vendor-primitive-architecture.md` designs most of the answer already; this section says which part of it is missing, and fills it.

### 7.1 — Three questions, usually conflated

**"I want a multiplier."** A portable capability. Works everywhere; better on some targets. This is exactly what the `Target` trait with default impls solves (`vendor-primitive-architecture.md` §3.2): the default method emits `assign p = a * b`, `Xilinx7Series` overrides it with a DSP48E1. Nothing about the widget is device-specific and nothing needs to be declared. **No addition needed.**

**"I want a clock manager."** A capability with no portable equivalent — there is no Verilog that synthesises into an MMCM. Also already designed: those trait methods return `Result<TargetEmit, UnsupportedPrimitive>` with a default of `Err`. The important property is *where* it fails. The widget compiles as Rust for any target; the failure arrives at `hdl_for(&target)`, structured, naming the primitive and the target. **No addition needed.**

**"I want `MUXF7`."** Not a capability. The author asked for that specific block, by name, because they want its timing or its placement or its cascade port. There is no abstraction to hide behind and there should not be one — and a trait cannot express it, because the trait would need a method per primitive and this library has two hundred of them. `Target` is the wrong shape for a named primitive, and named primitives are exactly what §3–§5 produce, two hundred at a time.

That third question is the gap, and it is the only one this section is about.

### 7.2 — A requirement is data, and the declaration already carries it

A named primitive belongs to a device family. `MUXF7` is Xilinx 7-series; `SB_MAC16` is iCE40. The declaration format already describes everything else known about a primitive, so this goes there too — and not per entry, because a library file is by construction a single family's worth of primitives. One line at the top of `xilinx-7series.ron`, stamped by the build script onto every entry it emits:

```ron
(
    source: "Vivado 2024.1 unisims",
    families: ["xilinx-7series"],
    generated_by: "tools/extract-unisim-connectivity",
    modules: [ ... ],
)
```

`Family` is a newtype over `&'static str` rather than an enum, so a third-party BSP can name its own silicon without patching `rhdl-core`:

```rust
/// A family of devices, named by the string a BSP and a target agree on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Family(pub &'static str);

impl Family {
    pub const XILINX_7SERIES: Family = Family("xilinx-7series");
    pub const LATTICE_ICE40: Family = Family("lattice-ice40");
}
```

A design's requirement is then the intersection of its parts'. The temptation is to store the intersection — a `&[Family]` that narrows as the tree is walked — and it is worth resisting, because the moment it is empty the diagnostic has nothing to say about *why*. Store the constraints instead and derive the intersection:

```rust
/// What silicon a design needs, as the list of things that decided it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Requirement {
    /// One entry per primitive that narrowed the requirement, in tree
    /// order. Empty means portable.
    constraints: Vec<Constraint>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Constraint {
    /// Hierarchical instance name, which is what the user needs to see.
    pub instance: String,
    /// The Verilog module that carries the requirement.
    pub module: &'static str,
    /// The families that provide it.
    pub families: &'static [Family],
}

impl Requirement {
    pub fn is_portable(&self) -> bool { self.constraints.is_empty() }
    pub fn permits(&self, f: Family) -> bool {
        self.constraints.iter().all(|c| c.families.contains(&f))
    }
    /// True when no family satisfies every constraint: a design no device
    /// can build, independent of which one was asked for.
    pub fn is_impossible(&self) -> bool { ... }
    /// Absorb a child's requirement, prefixing its instance names.
    pub fn absorb(&mut self, child: &Requirement, at: &ScopedName) { ... }
}
```

This is the same bottom-up-over-children shape as `combinational_reachability`, computed at the same point in descriptor finalisation, and it should reuse that traversal rather than adding a second one. It lives in `rhdl-core/src/circuit/portability.rs` and becomes a `Descriptor` field.

Note what it does *not* need: a generic parameter on the widget. The requirement is a property of the constructed tree, discovered by walking it, which is precisely what `architecture.md` §6 decision 10 asks for.

### 7.3 — Where it is checked

**At descriptor finalisation.** `is_impossible()` is an error immediately, with no target in hand. A design containing both a `MUXF7` and an `SB_MAC16` is wrong on its own terms and the earliest possible report is the right one.

**At `hdl_for(&target)`.** `permits(target.family())` is the main gate, and it is the one that catches the ordinary mistake.

**At `hdl()`.** Today there is no target and `hdl()` is the only emission path. It has to mean "emit for a portable target", which makes it an error for any design with a non-empty requirement — because there is no target to satisfy it, and silently emitting an instantiation nobody can elaborate is how this whole problem arises. That is a real migration cost and it has a concrete first casualty: `MuxF7`'s snapshot test calls `d.hdl()?`, and once §2 lets it instantiate a real `MUXF7` that call must become `d.hdl_for(&Xilinx7Series)?`. Every future primitive widget inherits the same shape, which is a good outcome — the test states the target the Verilog is for.

**The minimum `Target` this needs is two methods**, `name()` and `family()`. Not the capability surface of `vendor-primitive-architecture.md` §3.2, none of `PrimitiveRequest`, no NTL change. So this work can land on a trait skeleton well before that document's Phase 1 has any substance, and Phase 1 then fills the same trait in. Worth stating plainly, because the alternative reading — that device-specific code has to wait for the whole target system — would park the library in an unsound state for months.

### 7.4 — The diagnostic

The instance path is the payload. A user who wired a Xilinx primitive into a design four levels down does not need to be told that `MUXF7` is a Xilinx primitive; they need to be told where they put it.

```
Error: this design needs silicon the target does not provide
  target:    lattice-ice40
  needs one of: xilinx-7series

  because top.dut.mixer.sel instantiates MUXF7

  help: MUXF7 is a hard 7-series multiplexer with no equivalent on this
        target.  Either build for a 7-series part, or replace it with
        portable logic -- an ordinary `if`/`else` in the kernel lowers to
        a LUT mux on every target.
```

And the target-independent case, which names every witness because there is no single one to blame:

```
Error: no device can build this design
  top.dut.pll instantiates MMCME2_ADV  (xilinx-7series)
  top.dut.osc instantiates SB_HFOSC    (lattice-ice40)

  help: these two primitives exist on different silicon.  A design that
        needs both cannot be built at all; one of them has to become
        portable logic or move behind a target-selected leaf (see the
        book chapter on targets).
```

### 7.5 — Circuits that span devices

Now the part the requirement machinery exists to make safe. A widget that wants an MMCM on Xilinx and an `EHXPLLL` on ECP5 runs into a real tension: **the widget tree is built before the target arrives.** Widgets are Rust values, constructed by the user's program; the target is an argument to `hdl_for`. A widget therefore cannot choose its own children based on the target, because by the time the target exists the children are already there.

**Rejected: hold both children and let the emitter pick.** The descriptor tree, the netlist and the reachability matrix would all contain two clock managers, and — the fatal part — so would the Rust simulation, which has no target and so no basis for choosing. A `sim` that models one branch while `hdl_for` emits the other is exactly the Rust/Verilog divergence the whole five-tier stack exists to prevent.

**Accepted: choose the target at construction.** Widget construction is ordinary Rust and may branch freely:

```rust
let clocking = ClockManager::for_target(&target, ClockSpec { in_mhz: 100.0, out_mhz: 200.0 })?;
let dut = MyDesign { clocking, .. };
let hdl = dut.descriptor(ScopedName::top())?.hdl_for(&target)?;
```

The descriptor, the netlist and the simulation now all describe the same silicon, because one target built them. And the safety of it rests entirely on §7.3: without the requirement check, building for Xilinx and emitting for Lattice is silently wrong hardware. *That* is why the requirement is not merely a nicer error message — it is the interlock that makes construction-time target selection sound.

**Where the branch goes.** Not in a composed widget. A composed widget holding "either an MMCM child or an `EHXPLLL` child" needs an enum whose variants are different widget types, and since `Synchronous` has associated `I`/`O`/`D`/`Q`/`S`/`Kernel` types, that enum needs a hand-written `Synchronous` impl with enum `D`, `Q` and `S` types dispatching every method. Doable, tedious, and repeated per widget.

Put the branch at a **black-box leaf** instead, where the descriptor is hand-written anyway. A `ClockManager` is one widget with one `I` and one `O`, no children, one shared behavioural `sim`, and a `descriptor()` that picks a declaration and a body from the target it was built with — which is exactly the shape `MuxF7` already has. Composed widgets then never branch on target; they instantiate a leaf that does. That is the rule to write into the book chapter:

> Target-dependent primitive choice happens at a black-box leaf. Widgets above it are target-agnostic and stay that way.

**Does this stretch `architecture.md` §6 decision 10?** Worth answering rather than asserting compliance. Decision 10 says the target is a parameter to `hdl_for`, not a generic on widgets, and that widgets stay target-agnostic. The requirement machinery of §7.2 complies exactly: no generic, no type-level target, the property is discovered by walking the tree. A leaf built by `for_target` does not comply quite as cleanly — the target is not in its *type*, but it is in its *value*, and the widget's emitted Verilog now depends on something it was constructed with.

The type-level rule is the one with teeth, because it is what keeps target choice from propagating through every generic parameter of every composed widget, and it holds. The value-level exception is confined to black-box leaves, which are the only widgets that hand-write a descriptor anyway, and it is checked: the requirement the leaf records is what makes `hdl_for` refuse a mismatched target. Recording it as a scoped exception rather than pretending it is not one — and decision 10 gains a clause saying so.

**And the third case, which is most real designs.** A project supporting two boards usually does not want one widget that spans them; it wants two top-level designs sharing a portable core. That is a `match` in `main`, needs nothing from RHDL, and should be said out loud in the documentation so nobody reaches for §7.5's machinery when ordinary Rust will do.

### 7.6 — Why not Cargo features

Features are the first thing a Rust programmer reaches for, and `vendor-primitive-architecture.md` §10 already records that they are not the tool. The reasons are worth having concretely, because this library is where the temptation becomes acute:

- **Features are additive and unify across the graph.** If one dependency enables `xilinx` and another enables `lattice`, both are on. Mutual exclusion cannot be expressed, only detected and turned into a `compile_error!` — which is a feature system being used to emulate the enum it should have been.
- **One build is one configuration.** A multi-target design would need a build per target, so no single `cargo test --all` run could cover both, and the corpus cross-check — which has already caught three things it was not written for — would only ever see one.
- **RHDL compiles at run time.** The target is a value passed to `hdl_for`; a `#[cfg]` cannot be consulted there. This is the same reason the primitive library is ingested by a build script and then *carried as data*.

What features *are* right for: whether the generated library is compiled in at all. Two hundred entries of `const` data is compile time and binary size that a project targeting Lattice has no use for, and `default-features = false` plus a per-vendor feature is the correct mechanism for that. A build-size knob, not a correctness one — and the distinction is the whole point.

### 7.7 — What this costs the test contract

A widget wrapping a vendor primitive cannot have a Tier-4 `iverilog` round-trip in the ordinary way, because iverilog has no behavioural model of the primitive. `MuxF7` already lives with this: `sim` is the only executable description RHDL has of it. The stubs from §2 make `checked()` pass, which is a structural check — ports and widths — and no more.

There are three honest answers and the plan should use all three. `Target::sim_models` (`vendor-primitive-architecture.md` §3.2) is the designed hook for shipping hand-written iverilog-compatible models for the primitives RHDL actually lowers to, which restores Tier 4 for those. `xsim` against `unisims` (§6) restores it properly, on a Vivado machine, as an opt-in test. And for the rest, the widget's rustdoc says which tiers it has and why the missing one is missing — which is what CLAUDE.md §15 asks for anyway, and is better than a widget that quietly has four tiers where the contract says five.

### 7.8 — Tests

- A primitive widget reports the family its library declares, and the instance name in the constraint is its hierarchical name, not its module name.
- The requirement propagates through a parent that has no requirement of its own, and through two levels.
- Two conflicting primitives make `is_impossible()` true, and the diagnostic names both.
- `hdl_for` with a permitted target succeeds; with any other target it fails, and the message contains the instance path.
- `hdl()` on a design with a non-empty requirement fails rather than emitting.
- **A corpus check over the entire existing widget library asserting every descriptor is portable.** This is the valuable one, and the same shape as the reachability corpus check: it does not test the new code so much as guard against a widget acquiring a device dependency by accident, which is how a portable library stops being portable.

---

## 8 — Validation matrix

What is checked, by what, and what each check can and cannot establish.

| claim | checked by | can it fail wrongly? | can it miss a fault? |
|---|---|---|---|
| port names, directions, widths | extraction from `unisims` | no — it is the source | only if the extractor is wrong |
| port widths, again | stub vs instantiation mismatch in `checked()` | no | no, for widths used in a widget |
| a declared `None` is honest | simulation harness §4.2 | no — a change observed is a change | yes, if stimulus misses the path |
| a declared `Paths` is complete | nothing | — | yes, and knowingly |
| the primitive survives synthesis | **[VIVADO]** resource report | no | it may be inferred rather than instantiated |
| the whole design is consistent | existing corpus cross-check | no | no |
| a design names the silicon it needs | `Requirement` on the descriptor §7.2 | no | yes, if a library file's `families:` is wrong |
| a portable widget stayed portable | corpus check §7.8 | no | no |

Three rows are worth dwelling on. "A declared `Paths` is complete" is unchecked — a primitive with five real paths and four declared will under-report, and nothing catches it. And "the primitive survives synthesis" matters because Vivado may replace a hand-instantiated primitive with inferred logic; the resource report is the only way to know the instantiation had the effect intended. And "a design names the silicon it needs" rests on one line per library file, which nothing downstream can second-guess — the same believed-data problem as connectivity itself (§1), at a coarser grain and with a correspondingly smaller blast radius: getting `families:` wrong misdirects an error message, whereas getting connectivity wrong asserts a path does not exist.

---

## 9 — Phasing

Each of these is a PR. The first two block the others, and they block them for different reasons: A because nothing downstream can be built without it, B because everything downstream is unsound without it.

**A. External-module capability.** §2. No Vivado needed. `checked_with_stubs`, `HDLDescriptor::externals`, propagation up the hierarchy, `MuxF7` switched to a real instantiation, and a test that an *undeclared* module still fails. Roughly a day; the prototype is in §2.1.

**B. Portability requirements.** §7.2–§7.4. No Vivado needed. `Family`, `Requirement`, the `Descriptor` field computed alongside `combinational_reachability`, the two-method `Target` skeleton, `hdl_for`, both diagnostics, and the corpus check that every existing widget is portable. Must land immediately after A and before the library grows: the moment A ships, `MuxF7` instantiates real Xilinx silicon and claims nothing about it, and every entry stage C adds compounds that. Roughly two to three days.

**C. The extraction tool.** §3. **[VIVADO]** `tools/extract-unisim-connectivity/`, run by hand, output checked in. Emits every entry with proposed connectivity and marks anything attribute-dependent or unclassifiable as `Opaque` with a TODO note. The output of this stage is a complete but partly conservative library — which is already useful and already sound.

**D. The simulation harness.** §4.2. **[VIVADO]** Turns the conservative entries into verified ones, and — more importantly — can refute a wrong `None`. Run it over the whole library; every refutation is a bug in stage C's classifier and worth fixing there rather than patching the entry.

**E. Driver connectivity and fixture reachability.** §5. No Vivado needed. Independent of C and D.

**F. Examples.** §6, plus §7.5's target-selected leaf as a fourth. Needs A and B, and E for the third.

A pragmatic order if the Vivado machine is only available in bursts: do **A**, **B** and **E** anywhere, then **C** and **D** in one sitting at the Vivado machine, then **F**.

The multi-target machinery in §7.5 is deliberately *not* a phase. It needs no code beyond B — a constructor that takes a target is ordinary Rust — so it is a book chapter and an example, and it arrives with F. If it turns out to need a mechanism, that is a finding worth surfacing rather than a phase worth pre-planning.

---

## 10 — Risks and open questions

- **The extraction classifier will be wrong somewhere.** That is what §4.2 is for, and why C should run over the whole library rather than spot-checking. A classifier that is right for 190 primitives and wrong for 10 is not obviously distinguishable from one that is right for all 200, except by the harness.
- **`unisims` models are not always faithful to the silicon** on timing, and occasionally on corner-case behaviour. For the question being asked — is there a combinational path — they are the best available answer and are what Vivado's own simulator believes.
- **A Vivado version bump can change port lists.** The extraction output records the version; a bump means re-running B and diffing. The diff is the review.
- **Do the declarations belong in `rhdl-bsp`?** They are vendor data, and `rhdl-bsp` is the vendor-facing crate, so yes for now. If a second vendor's library arrives, the shape to reach for is one file per vendor under the same build script, not a new crate.
- **The `families:` line is believed the same way connectivity is.** A library file that claims 7-series when an entry is Ultrascale-only produces a design that passes the requirement check and fails in Vivado. Unlike connectivity there is no harness that can refute it, because the question is about a part RHDL cannot see. The mitigation is that it is one line per file rather than one per entry, reviewed once — and that stage C's extraction runs against a specific part, so the family is a property of the extraction rather than a judgement.
- **Does a `Requirement` belong to a widget or to a build?** §7.2 says the widget tree, because that is where the primitive is. But a project could reasonably want to declare "this crate is 7-series only" once, at the crate level, and have every widget in it inherit that. Crate-level declaration is not designed here and probably should not be: it invites a `families` key in `Cargo.toml`, which is the Cargo-features mistake of §7.6 in a new costume. Left open, with a bias against.
- **`hdl()` becoming fallible for primitive-wrapping widgets is a behaviour change** with a small blast radius today (one widget) and a large one after stage C. Landing B before C is what keeps it small; landing it after would mean touching every new primitive widget's tests twice.

- **Should `Opaque` entries be a build warning?** An `Opaque` entry is honest but pessimistic, and a library where half the entries are `Opaque` invites people to ignore the analysis. Counting them and printing the count at build time would keep the number visible without failing anything. Cheap; worth doing when the count is real.
