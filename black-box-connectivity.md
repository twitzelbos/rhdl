# Black-Box Connectivity

> **Status: design plan, not committed engineering work.** This document specifies how a module RHDL did not author — a vendor primitive, an IP core, a hand-written Verilog block — declares its combinational connectivity, so that the analyses in `rhdl-core::circuit::reachability` are sound in its presence. It defines the declaration API, an optional file format for bulk vendor libraries, the ingestion path for that format, and the migration required of the black boxes already in the tree.
>
> It exists because the reachability work (`combinational-reachability-and-loop-detection.md`, all three phases shipped) surfaced a gap it could not close: **every analysis in the compiler currently assumes a black box has no combinational feedthrough, and nothing checks that assumption.**

---

## 1 — How external modules get into an RHDL design today

Three mechanisms exist. They are genuinely different, none of them declares connectivity, and only one of them is visible to the compiler's analyses at all. Anyone reasoning about this needs all three in view.

### 1.1 — `Driver<T>`: the top-level I/O boundary

`rhdl-core/src/circuit/fixture.rs`, with implementations under `rhdl-bsp/src/drivers/` and `rhdl-bsp/src/ok/drivers/`.

```rust
pub struct Driver<T> {
    mounts: Vec<MountPoint>,        // Input(range) / Output(range) into the circuit's I / O
    ports: Vec<vlog::Port>,         // becomes top-level ports on the fixture
    pub hdl: vlog::ItemList,        // a Verilog *fragment*, not a module definition
    pub constraints: String,        // XDC/PCF text, emitted verbatim
}
```

A driver names top-level pins, emits a Verilog fragment that wires those pins to the circuit's interface, and carries the constraint text that pins them to package balls. `rhdl-bsp/src/drivers/xilinx/ibufds.rs` is the clearest example: it emits an `IBUFDS #(...) ibufds_clk(.O(inner_input[0:0]), .I(clk_p), .IB(clk_n));` instantiation. **RHDL never sees the `IBUFDS` module definition.** It is supplied by the vendor's simulation and synthesis libraries.

Two things follow, and both matter.

First, a driver is **not a circuit**. It has no `Descriptor`, no `ReachabilityMatrix`, and no place in the widget tree. It is assembled into the fixture after the circuit tree is complete. So no analysis in `rhdl-core` sees it.

Second, `mounts` is a `Vec` and `MountPoint` has both an `Input` and an `Output` variant, so **one driver may read circuit outputs and write circuit inputs**. That is not hypothetical: `rhdl-bsp/src/ok/drivers/xem7010/host.rs` mounts eleven inputs and seven outputs, routing them through the external `okHost` module. Whether that closes a *combinational* path depends on okHost's internals — it is a clocked host interface, so almost certainly not — but nothing in RHDL knows that, and nothing checks. A tristate or open-collector driver that drives a pin from an output and reads the same pin back into an input is the shape where this becomes real.

### 1.2 — `with_netlist_black_box()`: circuits whose Verilog RHDL writes by hand

`rhdl-core/src/circuit/descriptor.rs`. Used by `core::dff`, `core::ram::{synchronous,asynchronous}`, `cdc::{synchronizer,synchronizer_chain,slow_crosser}`, `reset::{conditioner,negation,negating_conditioner}`.

These *are* circuits. They have a `Descriptor`, they sit in the widget tree, and they are visible to every analysis. What makes them black boxes is that their kernel is `NoSynchronousKernel` — there is no `#[kernel]` to compile — and their Verilog is authored directly by the widget's own `hdl()` method via `parse_quote!`.

The netlist is a single opcode:

```rust
OpCode::BlackBox(BlackBox { lhs: <all output bits>, arg: vec![cr, i], code: BlackBoxId })
```

and the `Object` carries a `BlackBox { code: HDLDescriptor, mode: BlackBoxMode }` record alongside it.

### 1.3 — Vivado IP: build orchestration only

`rhdl-toolchains/src/vivado/tcl.rs` provides `CreateIp`, `ConfigureIp`, `GenerateIp`, which emit TCL:

```tcl
create_ip -name mig_7series -vendor xilinx.com -library ip -version 4.2 -module_name mig7
set_property CONFIG.BOARD_MIG_PARAM {Custom} [get_ips mig7]
generate_target all [get_files mig7.xci]
```

**There is no RHDL-circuit-level representation of a Vivado IP core at all.** Nothing in the type system, the netlist, or the descriptor tree knows the core exists. It is created by Vivado at build time and referenced by whatever Verilog fragment a driver happens to emit. This is the mechanism a user would reach for to instantiate a MIG, an FFT core, or a transceiver wrapper, and it is entirely outside the compiler.

### 1.4 — What the planned vendor-primitive system provides today: nothing

`vendor-primitive-architecture.md` specifies a `Target` trait, `Descriptor::hdl_for(&target)`, and a `primitive!` macro. **None of it is implemented.** There is no `trait Target` and no `hdl_for` in `rhdl-core`.

Note also that `architecture.md` §3 lists `PrimitiveRequest` among the opcodes in `ntl/spec.rs`. **That entry is stale** — no such opcode exists. Worth correcting when this work lands, because it is the kind of listing a reader trusts.

---

## 2 — The gap, stated precisely

`ntl::graph::make_net_graph` builds the dependency graph the combinational-path analyses walk. It contains this:

```rust
for (ndx, lop) in input.ops.iter().enumerate() {
    if matches!(lop.op, OpCode::BlackBox(_)) {
        continue;                    // <-- every black box, unconditionally
    }
    ...
}
```

Every `BlackBox` op is skipped when adding edges. **That is how a `DFF` breaks a combinational path**, and it is why `no_combinatorial_paths` and the composition-level cycle detector both give the right answer for a design full of registers.

It is an assumption about the black boxes that happen to exist, not a property any of them declares. Three facts make that uncomfortable:

- **`BlackBoxMode` is `Synchronous | Asynchronous`.** Neither means "combinational". The mode names the *circuit family* — whether the module takes an implicit `ClockReset` or carries its clocks inside `I` — and says nothing about feedthrough.
- **One black box in the tree already is combinational.** `reset::negation` is `assign o = ~i;`. It escapes being a counterexample only because it carries a `Reset`, and `GraphMode::Synchronous` deliberately excludes clock and reset from the graph. That is luck, not design.
- **The planned vendor primitives carry data.** `vendor-primitive-architecture.md` targets DSP slices, carry chains and LUT primitives. A combinational one — which is most of them — would make a real combinational path, and a real combinational *loop*, invisible to every check in the compiler.

The shipped reachability work made this sharper rather than safer. `ReachabilityMatrix::none()` is handed to every black box, and the composition-level cycle detector treats "no feedthrough" as fact. A combinational black box in a ring is a loop the compiler will now confidently tell you does not exist.

---

## 3 — What information is actually needed

Less than one might assume. The analyses ask exactly one question of a black box:

> For each input port and each output port, is there a combinational path between them?

That is a boolean matrix over ports — precisely `ReachabilityMatrix::i_to_o`, with the other three relations empty because a black box has no children.

Three further pieces of context are needed to make the answer usable:

1. **Which ports are clock or reset.** The analyses exclude them: a reset reaches every output by construction, so including it would make every matrix uniformly true and useless.
2. **How ports map to `I` and `O` fields.** The matrix is indexed by leaf field paths of the widget's types. A declaration in terms of Verilog port names has to be resolvable to those.
3. **Whether the declaration is complete.** A partial declaration must be distinguishable from a claim of no connectivity, because those have opposite safety properties.

Explicitly **not** needed for this work, and worth naming so scope does not drift: propagation delay, setup and hold times, clock-domain relationships, area, or power. Those belong to timing analysis and to `auto-pipelining-plan.md`. A combinational path either exists or it does not; how long it takes is a separate question with a separate consumer.

### 3.1 — Granularity

Port-level, which is field-level. The matrix stores field paths, and a Verilog port maps to a field.

The analysis computes at *bit* level internally and aggregates (see `combinational-reachability-and-loop-detection.md` §8, Phase 1), so a bit-level declaration could be honoured later without restructuring. It is not worth offering now: no vendor datasheet describes feedthrough per bit, and a declaration nobody can write accurately is worse than a coarser one everybody can.

---

## 4 — The design

Three layers. Only the first is required for soundness; the second is what makes it pleasant; the third is only worth building when someone has hundreds of primitives to describe.

### 4.1 — Layer 0: make the default sound

**The change with teeth, and the reason this document is not simply a feature proposal.**

Today an undeclared black box is assumed to have *no* feedthrough. That is the optimistic assumption, and optimism in a soundness analysis is how a real loop goes unreported. The default must invert: an undeclared black box is assumed to connect **every input to every output**.

That default is deliberately unpleasant. A widget that accepts it will be reported as having a combinational feedthrough, and a ring containing it will be reported as a combinational loop. Both reports would be conservative rather than wrong, and both are fixed by declaring the truth.

Inverting the default breaks every existing black box, which is the point: each of the nine widgets in §1.2 must state its answer. All nine are fully registered and will declare `none`, so the migration is mechanical — but it is a migration, and after it the assumption is recorded at each site instead of being inherited from a `continue` statement in a graph builder.

`ntl::graph::make_net_graph` changes correspondingly: instead of skipping every `BlackBox` op, it consults the declared connectivity and adds the edges the declaration names.

### 4.2 — Layer 1: declare it in Rust

The primary surface, and the source of truth. A black box is already declared in Rust, so its connectivity belongs there too.

```rust
/// Which inputs of a black box combinationally reach which outputs.
pub enum BlackBoxConnectivity {
    /// No input reaches any output: everything the module carries is
    /// registered.  What a DFF, a RAM, or a CDC synchroniser needs.
    None,
    /// Every input reaches every output.  The default for a module that
    /// has not said, and the honest answer for one nobody has analysed.
    Opaque,
    /// Exactly these pairs, and no others.
    Paths(Vec<(Path, Path)>),
}
```

and the black-box helper takes it, so the answer cannot be omitted:

```rust
impl Descriptor<SyncKind> {
    pub fn with_netlist_black_box(
        self,
        connectivity: BlackBoxConnectivity,
    ) -> Result<Descriptor<SyncKind>, RHDLError>;
}
```

The `DFF` migration is then one word:

```rust
Descriptor::<SyncKind> { /* ... */ }
    .with_netlist_black_box(BlackBoxConnectivity::None)
```

and a combinational primitive states its paths:

```rust
// A carry-chain adder: every sum bit depends on both operands, and the
// carry-out depends on everything.
.with_netlist_black_box(BlackBoxConnectivity::Paths(vec![
    (path!(i.a), path!(o.sum)),
    (path!(i.b), path!(o.sum)),
    (path!(i.carry_in), path!(o.sum)),
    (path!(i.a), path!(o.carry_out)),
    (path!(i.b), path!(o.carry_out)),
    (path!(i.carry_in), path!(o.carry_out)),
]))
```

`Paths` is verified against the widget's `I` and `O` kinds at descriptor-build time: a path that does not resolve is an error naming the port, not a silently ignored entry. That check is the reason to prefer `Path` over a bare string — a typo in a port name becomes a diagnostic rather than a missing edge, and a missing edge is exactly the failure this whole design exists to prevent.

### 4.3 — Layer 2: a file format, for bulk vendor libraries

Layer 1 is right for the handful of black boxes a project writes. It is wrong for a vendor primitive library: Xilinx alone has hundreds of primitives, their connectivity is a mechanical property of each, and the descriptions want to be generated, reviewed, diffed and shipped as data rather than hand-written as Rust.

**Format: RON.** Reasons, in order: it is already a `rhdl-core` dependency; it is comfortable to write and to review by hand, which matters for a file that will be read in diffs; and it distinguishes an enum variant from a struct without ceremony. The cost is one dependency line wherever the ingestion lives — see §5, where that turns out to be a build script and therefore not a workspace dependency at all. JSON is the alternative if avoiding RON matters more than legibility; `rhdl-macro-core` already carries `serde_json`.

One file describes one library of modules:

```ron
// xilinx-7series-primitives.ron
//
// Combinational connectivity for Xilinx 7-series primitives.
//
// Derived from UG953 (Libraries Guide) plus the simulation models in
// $XILINX_VIVADO/data/verilog/src/unisims.  Each entry records only
// whether a path is combinational, not how fast it is.
BlackBoxLibrary(
    // Provenance, so a reviewer can tell where a claim came from and a
    // regeneration can be checked against the same source.
    source: "Xilinx UG953 v2023.2 + unisims 2023.2",
    generated_by: "tools/extract-unisim-connectivity",

    modules: {
        // A differential input buffer: purely combinational.
        "IBUFDS": Module(
            ports: [
                Port(name: "I",  dir: Input,  width: 1),
                Port(name: "IB", dir: Input,  width: 1),
                Port(name: "O",  dir: Output, width: 1),
            ],
            connectivity: Paths([
                ("I",  "O"),
                ("IB", "O"),
            ]),
        ),

        // A flip-flop: nothing feeds through.  `C` is named as the clock
        // so that the analysis excludes it rather than reporting that it
        // reaches everything.
        "FDRE": Module(
            ports: [
                Port(name: "C",  dir: Input,  width: 1, role: Clock),
                Port(name: "R",  dir: Input,  width: 1, role: Reset),
                Port(name: "CE", dir: Input,  width: 1),
                Port(name: "D",  dir: Input,  width: 1),
                Port(name: "Q",  dir: Output, width: 1),
            ],
            connectivity: None,
        ),

        // A DSP slice, which is neither: some paths are registered and
        // some are not, and which is which depends on the attributes the
        // instantiation sets.  Declared for the fully-combinational
        // configuration, and flagged so that a reader knows the entry is
        // conditional on how it is instantiated.
        "DSP48E1": Module(
            ports: [
                Port(name: "CLK", dir: Input,  width: 1,  role: Clock),
                Port(name: "A",   dir: Input,  width: 30),
                Port(name: "B",   dir: Input,  width: 18),
                Port(name: "C",   dir: Input,  width: 48),
                Port(name: "P",   dir: Output, width: 48),
            ],
            connectivity: Paths([
                ("A", "P"),
                ("B", "P"),
                ("C", "P"),
            ]),
            // Free text, surfaced in the diagnostic when a path through
            // this module is reported, so the reader learns why.
            note: "Valid for AREG=BREG=CREG=PREG=0.  Any non-zero \
                   pipeline register attribute makes the corresponding \
                   path sequential; this entry is the conservative one.",
        ),

        // A module nobody has analysed.  Explicit, so that "not yet
        // described" is distinguishable from "described as having no
        // feedthrough" -- the distinction the whole design turns on.
        "MMCME2_ADV": Module(
            ports: [ /* ... */ ],
            connectivity: Opaque,
        ),
    },
)
```

Notes on the shape, each of which is a decision rather than a detail:

- **`connectivity` mirrors `BlackBoxConnectivity` exactly.** The file format is a serialisation of the Rust type, not a second model of the same idea. Two models drift.
- **`role` is optional and defaults to data.** Only `Clock` and `Reset` need naming, because only they are excluded from the analysis.
- **`Opaque` is spelled out rather than achieved by omission.** A module absent from the file and a module present with `Opaque` behave identically for the analysis, but they read completely differently to a maintainer: one is a gap, the other is a decision.
- **`source` and `generated_by` are mandatory.** A connectivity claim is a claim about silicon. Six months later the only question that matters about a wrong entry is where it came from.
- **`note` is surfaced in diagnostics.** When the loop detector reports a path through `DSP48E1`, the note about `PREG` is exactly what the user needs and exactly what they will not look up.

### 4.4 — What Layer 2 deliberately does not do

It does not read Verilog to *infer* connectivity. `rhdl-vlog` can parse Verilog text — `syn::parse_str::<ModuleDef>(...)` yields the module name and `Vec<Port>` with directions and widths — so extracting a module's *interface* mechanically is entirely feasible and is worth doing (§5.3). Inferring *connectivity* from a behavioural model is a different problem: it requires understanding `always` blocks, and getting it subtly wrong produces a confident, unsound answer, which is worse than requiring a human to write the entry. The parser gives us the ports for free; the paths are a judgement.

---

## 5 — Code infrastructure to ingest it

### 5.1 — Where the reading happens: a build script

The instinct is to read the file at descriptor-build time. That is wrong for this codebase, and the reason is worth being explicit about: **RHDL does its compilation at run time.** Descriptors are built when the program runs, so a path in a descriptor would be resolved relative to the working directory of whatever binary happened to call `descriptor()` — a test, an example, a synthesis driver. A missing or stale file would surface as a runtime error from deep inside descriptor construction, which is the worst place for it.

So the file is read at *compile* time by a build script in the crate that owns the primitives:

```
crates/rhdl-bsp/
├── build.rs                              # reads the .ron, emits Rust
├── primitives/
│   └── xilinx-7series.ron                # checked in, reviewed, diffed
└── src/
    └── drivers/xilinx/generated.rs       # `include!`d from OUT_DIR
```

`build.rs` deserialises the library, validates it, and writes a Rust module of `const` declarations into `OUT_DIR`. Failures are build failures with the offending module named. Nothing is read at run time, no path is resolved at run time, and the generated code is ordinary Rust that the compiler checks.

A build script rather than a proc macro, for three reasons. It needs no new crate and no new dependency edge — the `ron` dependency lives in `[build-dependencies]` of the crate that owns the primitives, so it never enters the workspace's runtime graph or raises the question in `architecture.md` §2 about what `rhdl-macro-core` may depend on. It is the conventional Rust answer to "generate code from a data file", so a contributor needs no RHDL-specific knowledge to follow it. And its output is a file on disk that can be read when something looks wrong, which a proc macro's expansion is not.

An ergonomic proc macro — `black_box_library!("primitives/xilinx-7series.ron")` — is a reasonable later addition for users writing their own primitive files. It should emit the same constructor calls the build script emits, so there is one code path being generated two ways rather than two implementations.

### 5.2 — What the generated code looks like

```rust
// Generated by build.rs from primitives/xilinx-7series.ron.  Do not edit.
pub const IBUFDS: BlackBoxDecl = BlackBoxDecl {
    module: "IBUFDS",
    ports: &[
        PortDecl { name: "I",  dir: Direction::Input,  width: 1, role: PortRole::Data },
        PortDecl { name: "IB", dir: Direction::Input,  width: 1, role: PortRole::Data },
        PortDecl { name: "O",  dir: Direction::Output, width: 1, role: PortRole::Data },
    ],
    connectivity: ConnectivityDecl::Paths(&[("I", "O"), ("IB", "O")]),
    note: None,
};
```

A `BlackBoxDecl` is port *names*; a `BlackBoxConnectivity` is field *paths*. Resolving between them is the widget author's job, at the one place that knows both: the widget maps its `I`/`O` fields onto the module's ports when it emits the instantiation, so it can resolve the declaration in the same breath.

```rust
impl<T: Digital> Synchronous for MyPrimitive<T> {
    fn descriptor(&self, name: ScopedName) -> Result<Descriptor<SyncKind>, RHDLError> {
        Descriptor::<SyncKind> { /* ... */ }
            .with_netlist_black_box(
                xilinx::IBUFDS.resolve(&[("I", path!(i.p)), ("IB", path!(i.n)), ("O", path!(o))])?
            )
    }
}
```

`resolve` is where the errors worth having live. It fails if a port in the declaration has no mapping, if a mapping names a port the declaration does not have, or if a mapped path does not resolve against the widget's kind. Each of those is a mistake someone will make, and each produces a wrong analysis rather than a visible fault if it is allowed to pass silently.

### 5.3 — Checking the declaration against the Verilog

Because `rhdl-vlog` can parse Verilog text, a declaration's *interface* can be checked mechanically against the module it describes, wherever the Verilog is available — a vendor's unisim model, a generated IP wrapper, a hand-written block:

```rust
#[test]
fn the_declarations_match_the_vendor_models() {
    for decl in xilinx::ALL {
        let Some(src) = unisim_source(decl.module) else { continue };
        let module: vlog::ModuleDef = syn::parse_str(&src).expect("parses");
        assert_ports_match(decl, &module);   // names, directions, widths
    }
}
```

This cannot check the *paths* — that is the judgement the file exists to record. But it catches the failure mode that will actually happen: a vendor renames a port or changes a width between tool versions, and a declaration that was right becomes quietly wrong. Ports are mechanical, so they should be checked mechanically, and the test should be skipped rather than failed where the vendor source is not installed.

---

## 6 — Migration

Nine widgets use `with_netlist_black_box()`. All nine are fully registered and declare `None`:

| widget | declares | why |
|---|---|---|
| `core::dff` | `None` | the register that everything else relies on to break paths |
| `core::ram::synchronous` | `None` | registered read port |
| `core::ram::asynchronous` | `None` | registered read port |
| `cdc::synchronizer` | `None` | two flops by construction |
| `cdc::synchronizer_chain` | `None` | N flops |
| `cdc::slow_crosser` | `None` | registered handshake |
| `reset::conditioner` | `None` | registered |
| `reset::negating_conditioner` | `None` | registered |
| `reset::negation` | **`Paths([(i, o)])`** | `assign o = ~i;` — the one that is genuinely combinational |

That last row is the interesting one and the reason the migration is worth doing rather than defaulting. `reset::negation` has always been a combinational black box, and the analysis has always treated it as a path breaker. It has never mattered, because it carries a `Reset` and the analysis excludes reset from the graph. Declaring it honestly costs nothing today and means the next reader learns the truth instead of inheriting the assumption.

Expected fallout: none. All nine declare what the analysis already assumed, so no snapshot, digest or diagnostic should move. **That expectation is the acceptance criterion for the migration** — if a Tier-3 HDL snapshot or a VCD digest changes, something was not what it appeared, and that is the finding rather than a nuisance.

---

## 7 — What this does not cover

Naming the boundaries, because each of these is a place where someone will reasonably expect this work to have helped and it will not have.

- **Drivers and the fixture (§1.1).** A driver is not a circuit, has no descriptor, and is assembled after the widget tree. The okHost driver's eleven inputs and seven outputs are outside every analysis, and this design does not bring them in. Doing so means giving the fixture a reachability matrix and treating drivers as edges in it — a coherent extension, and a separate one. Worth stating in the meantime that the largest external IP in the tree is the least analysed thing in it.
- **Vivado IP cores (§1.3).** A core created by TCL has no RHDL-level existence to attach a declaration to. Giving it one means a way to declare an IP core as a circuit — interface, connectivity, and the TCL to create it — which is the natural next piece of work after this one and is the thing a user asking "how do I use a Xilinx FFT core" actually needs.
- **Timing.** Whether a path is combinational, not how long it takes. `auto-pipelining-plan.md` owns the latter and will need more than this file provides.
- **Inferring connectivity from behavioural Verilog.** §4.4. The interface can be extracted; the paths are a judgement.
- **Multi-clock connectivity inside a black box.** A path from an input in one domain to an output in another is a CDC question, and the type system already handles domains at widget boundaries. A black box that crosses domains internally is a `Circuit`, and its declaration describes the same boolean matrix; whether the analysis should also record *which* domain each port belongs to is an open question below.

---

## 8 — Phasing

### Phase 1 — `BlackBoxConnectivity`, and the default inverted (1-2 weeks)

- `BlackBoxConnectivity` in `rhdl-core/src/circuit/reachability.rs`, beside the matrix it produces.
- `with_netlist_black_box` takes it; the nine call sites in §6 declare.
- `ReachabilityMatrix::none()` stops being what a black box gets by default; the declaration produces the matrix.
- `make_net_graph` consults the declaration instead of skipping every `BlackBox` op.
- Tests: a combinational black box is reported as a feedthrough; a ring containing one is reported as a loop; `None` behaves exactly as today.

**Acceptance:** no HDL snapshot, VCD digest or diagnostic changes for the nine existing black boxes, and a deliberately combinational black box is caught by both `no_combinatorial_paths` and the cycle detector — which it is not today.

### Phase 2 — `Opaque` as the default for an undeclared module (1 week)

Only meaningful once something can be undeclared, which needs Phase 3's file or a user's own primitive. Splitting it out keeps Phase 1's blast radius to the widgets in this tree.

### Phase 3 — the file format and the build script (2-3 weeks)

- The RON schema of §4.3, with serde types in a small crate or module shared between the build script and the runtime types.
- A build script in `rhdl-bsp` reading a checked-in `xilinx-7series.ron`, seeded with the primitives the BSP already instantiates — `IBUFDS` and the open-collector pattern to begin with, which makes the format earn its place on real cases before it is asked to describe hundreds.
- `resolve()`, and its three error cases.
- The interface cross-check of §5.3, skipped where the vendor source is absent.

### Phase 4 — IP cores as circuits (unscoped)

Sketched in §7. Wants its own document; it is a bigger question than connectivity, because it also has to answer what an IP core's Rust type is and who owns the TCL that creates it.

---

## 9 — Open questions

- **Should `Opaque` be an error rather than a conservative default?** A conservative default produces a confusing report — "combinational loop" through a module the user believes is registered — where a hard error would say "this module has not declared its connectivity" and point at the fix. The error is kinder and less flexible; the default lets a design build while the declaration is written. Leaning towards the error for a module a user declares themselves, and the conservative default for one ingested from a library.
- **Should a port's clock domain be part of the declaration?** §7. It would let a black box's CDC behaviour be checked rather than trusted, which is the same argument this whole document makes about connectivity. It is also a much larger surface.
- **Where do the serde types live?** A build script and the runtime both need them, and a build script cannot depend on the crate it is building. Either a tiny leaf crate — the `rhdl-dsp-design` precedent in `architecture.md` §7, which required a named structural constraint and two consumers that could not share a home, and this is arguably the same shape — or duplicated definitions with a test asserting they agree. The leaf crate is cleaner and costs a crate; the duplication is free and rots.
- **Does `BlackBoxMode` survive?** If connectivity is declared explicitly, `Synchronous | Asynchronous` is left describing only which circuit family the module belongs to, which is already implied by the descriptor's type marker. It may be redundant.

---

## 10 — Why this is worth doing before the vendor-primitive work

`vendor-primitive-architecture.md` is about *emitting* the right primitive for a target. This document is about *knowing what it does* once emitted. The two are independent, but the order matters: a DSP slice or a carry chain emitted into a design whose connectivity nobody declared is a combinational path the compiler will assert does not exist. Every analysis that shipped in `combinational-reachability-and-loop-detection.md` would then be confidently wrong in the one case the feature was introduced to handle.

Cheaper to make the declaration a precondition of the first primitive than to retrofit it across a library of them.
