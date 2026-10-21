# Full AXI4 for RHDL — Design Plan

**Status: the plan, with Phase 1's open questions answered by
experiment — see §8.1. No datapath yet.**

Validation is planned in four tiers, of which the first three are
reachable on the current toolchain and the fourth — interop against
vendor AXI IP — is not, and is designed for anyway. §7.5 states the
three constraints that keep it cheap to add later; they bind Phases 1
through 5.

Scope was set deliberately: **full conformance**, not a usable subset.
That means WRAP bursts, narrow and unaligned transfers, transaction IDs
with out-of-order completion, an ID-tracking interconnect, and the
pass-through QoS/cache/protection signals. It also means two
independent validation routes — an in-design protocol checker and a
hand-written Verilog monitor — rather than one.

This document exists because that scope is several PRs long and the
staging has to be agreed before the first line, per CLAUDE.md's rule
about never shipping a slice of a larger ask.

---

## 1 — Why full conformance, and what it commits us to

A usable subset (one master, INCR bursts, single ID, no switch) would
move DDR bandwidth and unblock roadmap item #60's PCIe bridge. It would
*not* let RHDL talk to third-party IP, because third-party IP issues
whatever the spec permits — and the parts a subset omits are exactly
the parts an unfamiliar master exercises first: a WRAP burst from a
cache controller, a narrow transfer from an 8-bit peripheral, two
outstanding IDs completing out of order.

**Decision recorded: conformance is the goal, and interop with IP we
did not write is the reason.** The cost is that several of the hardest
pieces (WRAP address arithmetic, out-of-order ID tracking) carry no
benefit for a design where RHDL is both ends. They are paid for
anyway.

The corollary matters as much: **a conformance claim needs an oracle
that is not us.** §7 is about that.

---

## 2 — The prerequisite the roadmap does not mention

`widget-roadmap.md` #56 says "AXI4-Lite becomes the degenerate
`Axi<32, 32, 0>` specialization". The existing `axi4lite` cannot become
that, because it is not generic over anything:

```rust
pub type AxilData   = Bits<32>;
pub type AxilAddr   = Bits<32>;
pub type AxilStrobe = Bits<4>;
```

105 uses across 18 files inside `axi4lite`, and about ten more files
outside it — `core::register_file`, both `rcstream::axi_stream`
bridges, four examples, `tests/loop_test.rs`.

So "unify Lite into AXI4" is a width-parameterisation refactor of ~28
files before any burst logic is written, with ~45 existing tests as the
safety net.

**Decision recorded: the two families coexist, and `axi4lite` is left
alone.** Reasons, in order of weight:

1. **The unification buys nothing a user can see.** A design using
   AXI4-Lite does not benefit from Lite being expressible as
   `Axi4<32, 32, 0>`; it benefits from Lite continuing to work.
2. **It would put a large mechanical refactor in front of the feature,
   and mechanical refactors are where regressions hide.** The 45 tests
   are a real net, but they are a net for *behaviour* — a
   parameterisation that compiles and passes is not obviously free of
   width-inference changes in the emitted Verilog.
3. **AXI4-Lite is a different protocol, not a special case, in one
   respect that matters**: it has no burst, so its endpoints need no
   beat counter and its interconnect needs no ID tracking. Expressing
   it through the AXI4 types means every Lite endpoint carries fields
   it must assert are constant. That is a conformance burden pointing
   the wrong way.

What this costs: two AXI families in the tree, and a reader having to
know which to use. Mitigated by saying so plainly in both modules'
docs, and by a one-direction bridge — `axi4::LiteBridge` presenting a
Lite endpoint to an AXI4 master — which is the composition anyone
actually wants and is cheap once AXI4 exists.

**If this decision is wrong, it is wrong in a recoverable direction:**
unifying later is the same refactor, no harder for having waited, and
by then AXI4 will have shown which parts of the shared surface are
real.

---

## 3 — The type layout

Five channels, as the spec has them. Each is an independent
ready/valid stream, which is what `axi4lite::channel::{sender,
receiver}` already implements generically over `T: Digital` and is
reused unchanged.

```rust
/// Address-phase beat, shared by AW and AR.
pub struct AxiAddress<const A: usize, const ID: usize> {
    pub id:     Bits<ID>,     // AxID
    pub addr:   Bits<A>,      // AxADDR
    pub len:    Bits<8>,      // AxLEN, beats - 1
    pub size:   Bits<3>,      // AxSIZE, log2 bytes per beat
    pub burst:  BurstKind,    // AxBURST
    pub lock:   bool,         // AxLOCK (AXI4 is 1 bit)
    pub cache:  Bits<4>,      // AxCACHE
    pub prot:   Bits<3>,      // AxPROT
    pub qos:    Bits<4>,      // AxQOS
    pub region: Bits<4>,      // AxREGION
}

pub enum BurstKind { Fixed, Incr, Wrap }   // 3 is reserved: unrepresentable
```

**Decision recorded: `AxBURST` is an enum with three variants, and the
reserved encoding is absent.** This is the same argument as
`roce::headers::WriteOpcode`: the illegal combination should not be
constructible. The wire decoder returns `Option<BurstKind>` and a
master presenting `0b11` is rejected with a named verdict rather than
coerced.

```rust
pub struct AxiWriteBeat<const D: usize> {
    pub data: Bits<D>,              // WDATA
    pub strb: Bits<{ D / 8 }>,      // WSTRB, one bit per byte lane
    pub last: bool,                 // WLAST
}

pub struct AxiReadBeat<const D: usize, const ID: usize> {
    pub id:   Bits<ID>,
    pub data: Bits<D>,
    pub resp: AxiResponse,          // RRESP, per beat
    pub last: bool,                 // RLAST
}

pub struct AxiWriteResponse<const ID: usize> {
    pub id:   Bits<ID>,
    pub resp: AxiResponse,          // BRESP, once per transaction
}

pub enum AxiResponse { Okay, ExOkay, SlvErr, DecErr }
```

`Bits<{ D / 8 }>` needs a `where rhdl::bits::W<{D/8}>: BitWidth` bound,
and const-generic expressions in bounds are the kind of thing that
either works cleanly or does not. **Open question, resolved in Phase 1
by trying it**: if it does not, the fallback is a separate `const
STRB: usize` parameter with a documented invariant `STRB == D / 8`
checked in the constructor, which is uglier and works.

---

## 4 — Burst arithmetic, which is where the bugs will be

The address of beat `n` is the part of AXI4 most often got wrong, and
the three burst kinds differ in ways that do not generalise.

Let `size = 1 << AxSIZE` (bytes per beat), `len = AxLEN + 1` (beats),
and `start = AxADDR`.

**FIXED.** Every beat uses `start`. Used for peripheral FIFOs where
the address is a port, not a location. `len` is capped at 16.

**INCR.** Beat `n` is at `aligned_start + n * size`, where
`aligned_start = start & !(size - 1)` for every beat *after* the
first; the first beat uses `start` itself and reduced strobes if
`start` is not size-aligned. `len` up to 256.

**The 4 KiB rule.** An INCR burst must not cross a 4096-byte boundary.
This is a *constraint on the master*, so an endpoint's job is to detect
and report it, not to handle it. It is the single most commonly
violated AXI4 rule in my experience of the literature, and it gets its
own checker assertion.

**WRAP.** `len` must be 2, 4, 8 or 16, and `start` must be
size-aligned. The burst wraps within a block of `len * size` bytes:

```
block_lo = start & !(len * size - 1)
addr(n)  = block_lo + ((start - block_lo) + n * size) % (len * size)
```

Both the length restriction and the alignment restriction are master
obligations; both get checker assertions.

**Narrow transfers.** `AxSIZE` smaller than the bus width means each
beat occupies a *rotating* subset of byte lanes, selected by the
beat's address modulo the bus width in bytes. This is the part that
cannot be tested by a loopback between two RHDL widgets that share an
off-by-one, which is why §7's independent oracle matters most here.

**Unaligned starts.** Only the first beat is affected, and only its
strobes.

---

## 5 — IDs and out-of-order completion

`AxID` is what makes AXI4 an interconnect protocol rather than a point
link. The rules that cost work:

- Read data for different IDs may interleave **beat by beat**. Read
  data for the *same* ID must stay in order.
- Write data may not interleave at all in AXI4 (AXI3's `WID` is gone),
  so the write path is simpler than the read path — a fact worth
  stating because it is a common source of over-engineering.
- Write responses may complete out of order across IDs.

So an endpoint needs, per outstanding ID, the beat counter and the
response accumulator; and the interconnect needs a table mapping
in-flight IDs to the port that issued them, plus the policy for when
that table is full.

**Decision recorded: the ID table is a fixed-depth direct-mapped
structure keyed on the low bits of the ID, and a collision stalls the
address phase rather than aliasing.** Exactly the shape — and the
reasoning — of `roce::qp::table` and `roce::qp::mr`: the alternative
to stalling is accepting a transaction whose response may be routed to
the wrong port, which is a data-corruption bug in an interconnect.
Stalling is a performance cost that shows up in a waveform; aliasing
is silent.

---

## 6 — The signals with no logic behind them

`AxCACHE`, `AxPROT`, `AxQOS`, `AxREGION` are carried, forwarded by the
interconnect, and otherwise ignored. `AxLOCK` is carried and, for
exclusive access, honoured by the endpoint that chooses to implement
it.

**Decision recorded: exclusive access is implemented at one endpoint
and nowhere else.** It is in scope because the scope is conformance,
but it is the lowest-value piece here — the monitor asserts the rules,
one endpoint demonstrates them, and nothing else in the tree uses it.
Said now so that nobody later reads its absence elsewhere as an
oversight.

---

## 7 — Validation: four tiers, because agreeing with yourself is not evidence

The RoCE work established the pattern worth repeating: a loopback
between two halves of the same crate proves they agree, not that they
are right. It became evidence only because both ends were independently
anchored to Scapy. AXI4 needs the same, and the oracle is harder to
come by.

### 7.1 — What is not available *yet*, stated up front

**ARM's AXI VIP is proprietary, and every open-source AXI checker I am
aware of is written in SystemVerilog assertions.** Icarus Verilog 13.0
accepts `-g2012` as a flag but does not implement SVA: no `property`,
no `assert property`, no `sequence`. Verilator and Yosys are not
installed on the development machine.

So the strongest external oracle is **not reachable today and is
planned for** — see §7.5, which is written now rather than later
precisely so the earlier tiers do not foreclose it. What would be the
worst outcome is claiming conformance on the strength of the tiers that
*are* reachable, and §7.5 states what each of them cannot establish.

### 7.2 — Tier A: an RHDL protocol checker, in the design

A synthesizable `axi4::check::Axi4Monitor` widget that observes all
five channels and reports violations as a typed verdict. Because it is
an RHDL widget it runs in the Rust simulator *and* through iverilog,
and it can be left in a real design behind a feature gate.

The assertions, each a named verdict variant:

- `AxVALID` must not be deasserted before `AxREADY` — once offered, a
  beat stays offered. The spec's strongest and most-violated handshake
  rule.
- `WLAST` is asserted on exactly the beat whose index is `AxLEN`, and
  on no other.
- Exactly one `BRESP` per accepted `AW`.
- `RLAST` once per read transaction per ID; read beats for one ID
  arrive in order.
- No INCR burst crosses a 4 KiB boundary.
- WRAP bursts have `len ∈ {2,4,8,16}` and a size-aligned start.
- `AxSIZE` never exceeds the bus width.
- `WSTRB` has no bits set outside the lanes the beat's address and size
  select.
- No write-data interleaving.

**This is the fast oracle and it is not independent**: it is RHDL
checking RHDL, and a shared misreading of the spec passes. That is what
Tier B is for.

### 7.3 — Tier B: a protocol monitor hand-written in Verilog

A plain-Verilog module — `always` blocks and `$error`, no SVA — that
implements the same assertion list, written from the spec rather than
from the RHDL checker, and instantiated in the iverilog testbench
alongside the RHDL-generated DUT.

**This is the independent oracle.** It is independent in the way that
matters: a different author pass, a different language, no shared code
with the design. It is weaker than vendor VIP in coverage and
weaker than SVA in expressiveness, and it is what is actually
achievable here.

**Open question, and the one real risk to this tier:** the existing
`SynchronousTestBench` generates a self-contained Verilog testbench,
and it is not yet established that an extra module can be injected
into it. If it cannot, the fallback is to emit the DUT's Verilog to a
file and drive iverilog directly from the test, outside the testbench
helper — more code in the test, same evidence. **Resolve this in
Phase 1, before building anything on top of it.**

### 7.4 — Tier C: the conformance suites

- **Directed**: one test per spec rule, each the minimal transaction
  that exercises it, plus the negative — a deliberately
  non-conformant master that the monitors must catch. Without the
  negatives the monitors are untested.
- **Randomised**: a structure-aware generator over (burst kind, len,
  size, start alignment, ID, interleaving), with a *deterministic
  seed* per CLAUDE.md §12.10, run against both monitors. The RoCE
  corpus established that uniform random fuzzing misses narrow
  windows; the generator solves for the interesting cases — 4 KiB
  boundary crossings, WRAP wrap-points, the narrowest transfer on the
  widest bus — rather than hoping to hit them.

### 7.5 — Tier D: vendor AXI IP, planned and not yet reachable

The decisive oracle, and the one that answers the question the other
three cannot: *does RHDL's AXI4 interoperate with IP written by someone
who has never seen it?* Tiers A–C can only show that RHDL agrees with
two readings of the spec, both produced here.

**Decision recorded: the earlier tiers are built so that this one slots
in without rework.** Concretely, three constraints on Phases 1–5:

1. **The DUT must be emittable as a standalone Verilog file with a
   plain AXI4 port list** — no RHDL-specific wrapper, no testbench
   entanglement. This is the same requirement §7.3's Verilog monitor
   imposes, which is why that question is answered in Phase 1: solving
   it for the hand-written monitor solves it for vendor IP too.
2. **Signal naming follows the AMBA convention** (`awvalid`, `awready`,
   `awaddr`, …) at the emitted boundary, not RHDL's field names. A
   vendor testbench binds by name, and a renaming shim is a place for
   a transcription error to hide.
3. **The stimulus generator of §7.4 must be able to drive an external
   DUT as well as an internal one**, so the same randomised corpus can
   be replayed against vendor IP rather than a new one being written
   for it.

What it needs, none of which is available here:

- **A simulator with real SystemVerilog support** — Verilator for
  open-source IP, or the vendor's own simulator for encrypted IP.
  Verilator is the cheaper half of this and worth installing first,
  since it also unlocks every open-source AXI checker.
- **IP to test against.** In rough order of how much each would prove:
  a vendor memory controller or interconnect (strongest — it is the
  real interop case); `pulp-platform/axi`'s `axi_test` package
  (open-source, SystemVerilog, has its own assertion suite); a vendor
  AXI VIP if a licence is to hand.
- **A machine with the vendor toolchain**, which
  `xilinx-primitive-library.md` already establishes as a separate
  environment and marks its steps `[VIVADO]`. Tier D's steps should
  carry the same marker for the same reason.

**Sequencing: Tier D comes after Phase 5 and does not gate it.** A
conformance failure found against vendor IP is a bug in RHDL's AXI4
that the earlier tiers missed, and finding it later costs a fix, not a
redesign — provided the three constraints above were honoured. Those
constraints are the whole reason this section exists before the code
does.

---

## 8 — Staging

Each phase is a PR, each is independently reviewable, and each has a
gate that does not depend on the next.

**Phase 1 — types, channels, and the validation spine.** The beat
structs, `BurstKind`/`AxiResponse` with their wire codecs proved
exhaustively inverse (the RoCE `decode_write_opcode` pattern), the
`Bits<{D/8}>` question resolved, and *both monitors built against a
deliberately non-conformant master*. Nothing that moves data.
*Gate: the monitors catch every violation in a hand-written
non-conformant generator, and the Verilog-injection question is
answered.*

**Phase 2 — single-ID master and endpoint, INCR and FIXED.** Beat
counters, `WLAST`/`RLAST`, `BRESP`. *Gate: byte-exact loopback across
every `len` 1..256 and every `size` up to the bus width, with both
monitors silent.*

**Phase 3 — WRAP, narrow, unaligned.** The arithmetic of §4. *Gate:
the randomised suite from §7.4 against both monitors, including the
boundary cases the generator solves for.*

**Phase 4 — IDs and out-of-order.** Per-ID state, the ID table of §5,
read-beat interleaving. *Gate: two IDs completing out of order, and a
full ID table stalling rather than aliasing — asserted as a
distinguishable outcome, not merely as "it still works".*

**Phase 5 — the interconnect.** ID-to-port tracking, arbitration.
*Gate: three masters and two endpoints, every transaction landing at
the right port with responses routed back correctly under randomised
interleaving.*

**Phase 6 — exclusive access, and `axi4::LiteBridge`.** The lowest-value
conformance piece and the composition that makes Lite's coexistence
tolerable.

**Phase 7 — Tier D, vendor IP interop.** Hardware- and
toolchain-gated, per §7.5: needs Verilog or SystemVerilog IP to test
against and a simulator that can run it. Does not gate Phases 1–6, and
the three constraints in §7.5 are what keep it cheap when it arrives.
*Gate: a transaction stream generated by RHDL's master is accepted by
vendor IP, and vendor-generated traffic is served correctly by RHDL's
endpoint, under the randomised corpus of §7.4.*

---

## 8.1 — Phase 1's open questions, answered

Resolved by experiment before any datapath was written, which is what
Phase 1 is for. Two of the three change a premise above.

### `Bits<{ D / 8 }>` is not expressible, and the fallback works

`struct WriteBeat<const D: usize> { strb: Bits<{ D / 8 }> }` does not
compile on stable:

```
error: generic parameters may not be used in const operations
  = help: const parameters may only be used as standalone arguments
```

It needs `generic_const_exprs`, which is unstable. The precedent in
`dsp::cic::pruned` is not a counterexample — that expression sits
inside a `macro_rules!` expansion where the parameters substitute to
literals, so it is never a *generic* const operation.

**Resolution: the §3 fallback, confirmed working.** A separate
`const STRB: usize` with the invariant `STRB * 8 == D` asserted in a
`const fn check()`, which a caller can force at compile time with
`const _: () = WriteBeat::<32, 4>::check();`. Verified for a 32-bit
bus and a 128-bit one.

### The emitted module boundary has no named ports, and that is the real risk

The plan guessed the Verilog-monitor risk was *injecting a module into
the generated testbench*. It is not — `TestModule` is a `ModuleList`
with a `Display`, so writing a bespoke testbench that instantiates DUT
and monitor together is trivial and needs no new machinery.

The actual risk is one level down. RHDL emits a widget's `I` and `O` as
**single packed vectors**:

```verilog
module dut(input wire [1:0] clock_reset, input wire [7:0] i, output reg [7:0] o);
```

So an AXI4 widget does not present `awvalid`, `awaddr`, `awready`. It
presents one wide `i` and one wide `o`. Nothing in `TestBenchOptions`
or `hdl()` changes this.

That breaks §7.5's constraint 2 as written, and it matters for both
oracles: a hand-written monitor would have to slice the packed vectors
at offsets transcribed from RHDL's `Digital` layout, and a vendor
testbench — which binds by name — cannot bind at all.

### …but the wrapper can be generated, not transcribed

`rhdl_core::types::path::bit_range(kind, path)` is public and returns
the exact bit range of a named field:

```
.valid -> bits 0..1    .addr -> bits 1..33   .len -> bits 33..41
.size  -> bits 41..44  .id   -> bits 44..48
```

The leaf ranges tile the struct with no gaps or overlaps, which is what
makes this safe rather than merely possible.

**Resolution, and an amendment to §7.5: an AMBA-named port wrapper is
generated from the `Digital` type, and generating it is a Phase 1
deliverable rather than a Tier-D afterthought.** Three reasons it
belongs there:

1. Both oracles need it. Tier B's monitor should bind to named ports
   for the same reason Tier D's vendor IP must — a hand-transcribed
   offset table is a defect waiting for a struct-field reorder.
2. It is **general infrastructure, not AXI4's**. Any widget that has to
   present a named Verilog boundary wants it: vendor primitives per
   `xilinx-primitive-library.md`, the black boxes of
   `black-box-connectivity.md`, the MAC frontend `net` is waiting for.
   Building it inside `axi4` would be the wrong home.
3. It must be built through `rhdl-vlog`'s AST, not by string
   templating — `architecture.md` is explicit, and a five-channel AXI4
   boundary is far too many slices to hand-write safely in any case.

**Open question this raises, for decision before Phase 1 proceeds:**
whether the wrapper generator lands as its own change first — it is
reusable, it has users other than AXI4, and it is the sort of thing
that gets worse if it grows up inside one consumer — or as part of
Phase 1 with a note to extract it later. The first is cleaner and
delays AXI4 by one PR.

---

## 9 — Risks and open questions

1. ~~**`Bits<{ D / 8 }>` may not be expressible.**~~ **Answered: it is
   not.** See §8.1. The fallback is confirmed working.
2. ~~**The Verilog monitor may not be injectable into the generated
   testbench.**~~ **Answered, and the guess was wrong.** Injection is
   trivial; the real obstacle is that RHDL emits struct I/O as packed
   vectors with no named ports, so both oracles need a generated
   AMBA-named wrapper. See §8.1. This did change the shape of the work,
   which is why it was first.
3. **A hand-written Verilog monitor is a second implementation, and
   second implementations have their own bugs.** Mitigated by the
   deliberately non-conformant master: a monitor that cannot catch a
   known violation is a broken monitor, and both tiers are held to
   that.
4. **256-beat bursts make simulation long.** The randomised suite will
   need a time budget per case and a committed corpus rather than
   fresh randomness per run, as `roce`'s fuzz corpus does.
5. **Coexisting with `axi4lite` will confuse someone.** Mitigated by
   docs and the bridge; revisit unification once AXI4 exists and the
   shared surface is known rather than guessed.
6. **The kernel subset has bitten every protocol widget so far** — no
   `if let`, no match guards, `resize` truncating rather than
   saturating, `Bits<N>` only for N ≤ 128. A 512-bit AXI4 data bus is
   **wider than `Bits<128>`**, so wide-bus support needs either an
   array-of-words payload or a kernel-language change. **Open
   question, and it bounds what "full" can mean**: resolve the
   intended maximum bus width in Phase 1 before the beat structs are
   fixed.
