# The Xilinx Primitive Library

> **Status: an execution plan, to be carried out on a machine with Vivado installed.** This document specifies how to build a complete black-box declaration library for the Xilinx 7-series — the primitives a Zynq-7020 provides — how to *verify* the connectivity claims by simulation rather than merely asserting them, and what has to exist in `rhdl-core` first.
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

## 7 — Validation matrix

What is checked, by what, and what each check can and cannot establish.

| claim | checked by | can it fail wrongly? | can it miss a fault? |
|---|---|---|---|
| port names, directions, widths | extraction from `unisims` | no — it is the source | only if the extractor is wrong |
| port widths, again | stub vs instantiation mismatch in `checked()` | no | no, for widths used in a widget |
| a declared `None` is honest | simulation harness §4.2 | no — a change observed is a change | yes, if stimulus misses the path |
| a declared `Paths` is complete | nothing | — | yes, and knowingly |
| the primitive survives synthesis | **[VIVADO]** resource report | no | it may be inferred rather than instantiated |
| the whole design is consistent | existing corpus cross-check | no | no |

Two rows are worth dwelling on. "A declared `Paths` is complete" is unchecked — a primitive with five real paths and four declared will under-report, and nothing catches it. And "the primitive survives synthesis" matters because Vivado may replace a hand-instantiated primitive with inferred logic; the resource report is the only way to know the instantiation had the effect intended.

---

## 8 — Phasing

Each of these is a PR. The first is the only one that blocks the others.

**A. External-module capability.** §2. No Vivado needed. `checked_with_stubs`, `HDLDescriptor::externals`, propagation up the hierarchy, `MuxF7` switched to a real instantiation, and a test that an *undeclared* module still fails. Roughly a day; the prototype is in §2.1.

**B. The extraction tool.** §3. **[VIVADO]** `tools/extract-unisim-connectivity/`, run by hand, output checked in. Emits every entry with proposed connectivity and marks anything attribute-dependent or unclassifiable as `Opaque` with a TODO note. The output of this stage is a complete but partly conservative library — which is already useful and already sound.

**C. The simulation harness.** §4.2. **[VIVADO]** Turns the conservative entries into verified ones, and — more importantly — can refute a wrong `None`. Run it over the whole library; every refutation is a bug in stage B's classifier and worth fixing there rather than patching the entry.

**D. Driver connectivity and fixture reachability.** §5. No Vivado needed. Independent of B and C.

**E. Examples.** §6. Needs A, and D for the third.

A pragmatic order if the Vivado machine is only available in bursts: do **A** and **D** anywhere, then **B** and **C** in one sitting at the Vivado machine, then **E**.

---

## 9 — Risks and open questions

- **The extraction classifier will be wrong somewhere.** That is what §4.2 is for, and why C should run over the whole library rather than spot-checking. A classifier that is right for 190 primitives and wrong for 10 is not obviously distinguishable from one that is right for all 200, except by the harness.
- **`unisims` models are not always faithful to the silicon** on timing, and occasionally on corner-case behaviour. For the question being asked — is there a combinational path — they are the best available answer and are what Vivado's own simulator believes.
- **A Vivado version bump can change port lists.** The extraction output records the version; a bump means re-running B and diffing. The diff is the review.
- **Do the declarations belong in `rhdl-bsp`?** They are vendor data, and `rhdl-bsp` is the vendor-facing crate, so yes for now. If a second vendor's library arrives, the shape to reach for is one file per vendor under the same build script, not a new crate.
- **Should `Opaque` entries be a build warning?** An `Opaque` entry is honest but pessimistic, and a library where half the entries are `Opaque` invites people to ignore the analysis. Counting them and printing the count at build time would keep the number visible without failing anything. Cheap; worth doing when the count is real.
