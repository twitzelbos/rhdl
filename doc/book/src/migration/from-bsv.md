# Migrating from Bluespec SystemVerilog to RHDL

This chapter is for **Bluespec SystemVerilog (BSV) users** evaluating RHDL.
You already understand guarded atomic rules, the conflict matrix, and the
Bluespec scheduling model. This chapter shows you the RHDL spelling for
every BSV idiom and walks through one worked example end-to-end.

If you are not coming from Bluespec, skip this chapter — the regular RHDL
chapters (and `rule-architecture.md` in the repository root) cover everything
without assuming BSV background.

---

## Why this chapter exists

Bluespec proved that guarded atomic rules + compiler-synthesized scheduling
eliminate four chronic RTL problems: manual scheduling, implicit race
conditions, FSM explosion, and poor composability.[^bluespec] After two
decades of academic and commercial use, the abstraction is well validated;
the limitation that has held it back is the language ecosystem — proprietary
compiler, proprietary IDE, no `cargo` equivalent, no `crates.io`, no
`docs.rs`.

`rhdl-rule` ports the *semantic model* (guarded atomic rules, conflict-driven
scheduling, atomic commit) into Rust-embedded RHDL. The result is the
strongest combination on offer: Bluespec's correctness story, RHDL's
clock-domain typing, and Rust's tooling.

This chapter is your translation table. Open it with your Bluespec source
code on one side and write the RHDL version on the other.

---

## At-a-glance translation table

| Bluespec idiom | RHDL equivalent | Notes |
|---|---|---|
| `module mkFoo` | `pub struct Foo { … }` + `rule_kernel! { … }` or `#[rule_kernel_attr] impl Foo { … }` | Both surface forms produce byte-identical kernels. |
| `Reg#(t) reg <- mkReg(init);` | `reg: dff::DFF<T>` (or `reg: Reg<T>` from `rhdl-rule-rt`) | Init values are set in `Default::default()` for the wrapping struct. |
| `FIFOF#(t)` | `rhdl_fpga::fifo::SyncFIFO<T, N>` | RHDL has many FIFO variants — sync, async, distributed. |
| `mkConnection(a, b)` | Wire ports together by hand in the parent kernel; or use `RCStream<T, F, D>` for typed latency-insensitive bus connection. | RHDL's `RCStream` is the typed equivalent of an AXI-Stream / mkConnection-like channel. |
| `rule bump (cond);` | `#[rule] fn bump(ctx: &mut RuleCtx<Self>, i: I) { guard!(cond); … }` | Guard expressions compose via `&&`. |
| `reg <= value;` (non-blocking write) | `ctx.reg = value;` (canonical) — *or* `set!(ctx.reg, value)` (legacy) | Same semantics: non-blocking, atomic at cycle boundary. The operator changes; the meaning doesn't. |
| `let x = expr;` (combinational let) | `let x = expr;` (preamble) | Hoisted into a per-rule block scope so multiple writes can share. |
| `rule_attribute (descending_urgency = "a, b")` | `#[rule(urgent_before = "b")]` on rule `a` | Pairwise edges instead of a list; the macro topo-sorts. |
| `rule_attribute (mutually_exclusive = "a, b")` | `#[rule(mutually_exclusive = "b")]` on rule `a` (or vice versa — symmetric) | Trusts the assertion; elides the suppressor in the priority chain. |
| `rule_attribute (conflict_free = "a, b")` | `#[rule(conflict_free = "b")]` on rule `a` | Validates against the computed conflict matrix; rejected if the pair actually conflicts. |
| `rule_attribute (preempts = "a, b")` | Use explicit `priority = N` plus `urgent_before` | RHDL's preempts equivalent is the priority-chain ordering. |
| Explicit module instantiation | Sub-circuit field on the parent struct | Auto-derived `D`/`Q` types wire it up. |
| BSV's implicit-condition method-call ready signal | Not in v1 (cross-module method semantics out of scope per `rule-architecture.md` §17.1). | The ready-signal pattern can be expressed manually via `RCStream` handshakes when needed. |
| `mkPulseWire` | `dff::DFF<bool>` written `false` by default and `true` only by the producing rule | The "pulse" is a one-cycle write; subsequent cycles read `false` if no writer fires. |
| `interface IFoo { method … endmethod }` | `In` / `Out` aggregate types + the `#[output]` method | The output method is RHDL's "value method"; v1 has no equivalent of multi-method interfaces. |

The rest of this chapter expands the rows that need more than a sentence.

---

## 1 — Module definition

### Bluespec

```bsv
module mkCounter (Reg#(Bit#(8)) ifc);
   Reg#(Bit#(8)) count <- mkReg(0);

   rule bump;
      count <= count + 1;
   endrule

   return count;
endmodule
```

### RHDL (function-like form)

```rust
use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel;

rule_kernel! {
    pub struct Counter {
        count: dff::DFF<Bits<8>>,
    }

    impl Counter {
        #[rule]
        fn bump(ctx: &mut RuleCtx<Self>, _i: bool) {
            ctx.count = *ctx.count + bits::<8>(1);
        }

        #[output]
        fn output(self_q: &Self, _i: bool) -> Bits<8> {
            *self_q.count
        }
    }
}
```

### RHDL (attribute form)

```rust
#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct Counter {
    count: dff::DFF<Bits<8>>,
}

#[rule_kernel_attr]
impl Counter {
    #[rule]
    fn bump(ctx: &mut RuleCtx<Self>, _i: bool) {
        ctx.count = *ctx.count + bits::<8>(1);
    }

    #[output]
    fn output(self_q: &Self, _i: bool) -> Bits<8> {
        *self_q.count
    }
}
```

Both forms produce byte-identical kernels; pick whichever reads more
naturally for the widget. See `rule-architecture.md` §4.5 for the full
trade-off discussion.

### Differences worth noting

- **Inputs are explicit.** BSV implicitly has access to `count` via lexical
  scope. RHDL's `#[rule]` takes `ctx: &mut RuleCtx<Self>` (the register
  accessor) and an input parameter. This makes the rule's read/write
  surface visible at the type level.
- **Output is a separate method.** BSV's `return count;` returns an
  interface that exposes the register. RHDL's `#[output]` method is the
  explicit value-method equivalent — runs combinationally after all rules
  fire each cycle, computes the widget's `Out` from the post-firing state.
- **The struct is the module.** No separate "interface" / "implementation"
  split.

---

## 2 — Register writes: `<=` vs `=`

This is the **single biggest visual difference** for BSV users. BSV uses
`<=` for non-blocking register writes; RHDL uses `=`.

### Bluespec

```bsv
rule update;
   reg_a <= reg_a + 1;
   reg_b <= reg_b * 2;
endrule
```

### RHDL

```rust
#[rule]
fn update(ctx: &mut RuleCtx<Self>, _i: bool) {
    ctx.reg_a = *ctx.reg_a + bits::<8>(1);
    ctx.reg_b = *ctx.reg_b * bits::<8>(2);
}
```

### Why the spelling change

In Rust, `<=` is the comparison operator returning `bool`. Overloading it
inside a macro to mean "non-blocking write" would produce code that visually
parses as a Boolean comparison — a constant footgun. RHDL uses Rust-native
`=`; the atomicity is guaranteed by the **scope** the assignment appears in
(a `#[rule]` method body), not by the operator.

The `RuleCtx<Self>` type around `ctx` is a phantom-typed marker — it has no
runtime fields. So `ctx.field = value` cannot possibly be a regular Rust
mutation; the macro is the only thing giving it meaning. Readers familiar
with the phantom-type pattern see immediately that the assignment is
metadata, not an imperative side effect.

### Mental model

- BSV's `=` means combinational `let`-binding (substituted in place).
- BSV's `<=` means non-blocking register write (atomic at clock edge).
- RHDL's `let x = expr;` *inside a rule body* means combinational
  let-binding (the per-rule preamble — see §3 below).
- RHDL's `ctx.field = expr;` *inside a rule body* means non-blocking
  register write.

The mapping is direct, just spelled with the operator Rust users expect.

---

## 3 — Combinational let-bindings (per-rule preamble)

### Bluespec

```bsv
rule fifo_step;
   Bool full       = (write_addr + 1) == read_addr;
   Bool will_write = write_enable && !full;

   write_addr         <= will_write ? write_addr + 1 : write_addr;
   overflow           <= overflow || (write_enable && full);
   write_addr_delayed <= write_addr;
endrule
```

`full` and `will_write` are computed once and used in three writes.

### RHDL

```rust
#[rule]
fn fifo_step(ctx: &mut RuleCtx<Self>, i: PreambleFifoIn<N>) {
    // Preamble — shared computation visible to every write.
    let full       = (*ctx.write_address + bits::<N>(1)) == i.read_address;
    let will_write = i.write_enable && !full;

    // Three non-blocking writes referencing the preamble.
    ctx.write_address         = if will_write { *ctx.write_address + bits::<N>(1) } else { *ctx.write_address };
    ctx.overflow              = *ctx.overflow || (i.write_enable && full);
    ctx.write_address_delayed = *ctx.write_address;
}
```

The macro recognizes the `let` bindings as the rule's **preamble** and
hoists them into a per-rule block scope so all three direct-assignments see
the same pre-computed values. Same combinational semantics as BSV's `=`
inside a rule.

---

## 4 — Rules with guards

### Bluespec

```bsv
rule increment_when_enabled (enable && !at_max);
   count <= count + 1;
endrule
```

The condition in parentheses after `rule X` is the guard.

### RHDL

```rust
#[rule]
fn increment_when_enabled(ctx: &mut RuleCtx<Self>, enable: bool) {
    guard!(enable);
    guard!(*ctx.count != bits::<8>(255));
    ctx.count = *ctx.count + bits::<8>(1);
}
```

Multiple `guard!` calls in the same rule body are conjoined by `&&`. There
is no semantic difference between `guard!(a && b)` and `guard!(a); guard!(b);`.

---

## 5 — Annotations: `descending_urgency`, `mutually_exclusive`, `conflict_free`

### Bluespec

```bsv
(* descending_urgency = "rule_a, rule_b" *)
(* mutually_exclusive = "rule_a, rule_b" *)
(* conflict_free      = "rule_a, rule_b" *)
```

BSV uses module-level attributes that take a comma-separated list of rule
names.

### RHDL

```rust
#[rule(urgent_before = "rule_b")]
fn rule_a(ctx: &mut RuleCtx<Self>, i: I) { … }

#[rule(mutually_exclusive = "rule_b")]
fn rule_a(ctx: &mut RuleCtx<Self>, i: I) { … }

#[rule(conflict_free = "rule_b")]
fn rule_a(ctx: &mut RuleCtx<Self>, i: I) { … }
```

RHDL annotations are pairwise on individual rules. The macro:

- **Topo-sorts** `urgent_before` edges (cycles, self-loops, and meaningless
  edges between non-conflicting rules are compile errors).
- **Validates** `conflict_free` assertions against the computed conflict
  matrix (rejected if the pair actually conflicts).
- **Trusts** `mutually_exclusive` as a scheduler-optimization hint, eliding
  the redundant `&&!(_fire_other)` suppressor in the priority chain. This
  matches BSV's behaviour: the assertion is taken on faith.

For multi-rule sets, repeat the annotation:

```rust
#[rule(mutually_exclusive = "rule_b", mutually_exclusive = "rule_c")]
fn rule_a(...) { ... }
```

---

## 6 — Per-rule debugging: `#[rule(trace)]`

BSV exposes rule-firing decisions through the Bluesim simulator's
introspection API. RHDL adds an opt-in equivalent via the `trace`
annotation:

```rust
#[rule(trace)]
fn bump(ctx: &mut RuleCtx<Self>, _i: bool) {
    ctx.count = *ctx.count + bits::<8>(1);
}
```

When set, the macro emits visible `let fire_<rule>` and `let can_fire_<rule>`
bindings (no underscore prefix) so the rule's firing decisions show up in
VCD waveforms. Off by default — kernels you're not actively debugging stay
lean. Composes freely with the other annotations:

```rust
#[rule(priority = 0, trace)]
fn high_pri_rule(...) { ... }
```

---

## 7 — Composition: rule kernels + traditional widgets

BSV modules compose via interfaces. RHDL widgets compose via sub-circuit
fields:

### Bluespec

```bsv
module mkMonitoredArbiter (...);
   PriorityArbiter arb <- mkPriorityArbiter;
   Reg#(Bit#(32)) grant_count <- mkReg(0);

   rule count_grants;
      if (arb.grant matches tagged Valid .g)
         grant_count <= grant_count + 1;
   endrule

   …
endmodule
```

### RHDL

```rust
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct MonitoredArbiter {
    arbiter:     PriorityArbiter<4, 2>,   // rule-kernel sub-circuit
    grant_count: dff::DFF<Bits<32>>,      // traditional sub-circuit
}

#[kernel]
pub fn monitored_arbiter_kernel(
    cr: ClockReset,
    requests: Bits<4>,
    q: Q,
) -> (MonitoredArbiterOut, D) {
    let mut d = D::dont_care();
    d.arbiter = requests;

    let grant: Option<Bits<2>> = q.arbiter;
    d.grant_count = q.grant_count + match grant {
        Some(_) => bits::<32>(1),
        None    => bits::<32>(0),
    };
    …
}
```

The wrapper is a hand-written `Synchronous` widget; it contains a
rule-kernel sub-circuit (`PriorityArbiter`) and a traditional sub-circuit
(`dff::DFF`). The wrapper's hand-written kernel reads `q.arbiter` (the
rule-kernel's output), drives `d.arbiter` (its input), and updates the
traditional grant counter.

This is the canonical pattern when a rule-shaped sub-design lives inside a
larger non-rule-shaped widget. See `rule-architecture.md` §9.1 and the
`pilot_composition.rs` test for a complete worked example.

---

## 8 — When *not* to use rule kernels

BSV users sometimes assume "rules everywhere, always." That's not the right
default in RHDL.

A widget is naturally one rule (and benefits zero from the rule-kernel
abstraction) when **everything happens together every cycle**. Examples:

- A FIFO write-side state machine that always advances the pointer when
  there's room, always latches overflow when there isn't, and always
  propagates the pointer to a delayed copy. The `pilot_fifo_write_logic.rs`
  retrospective walks through this — the obvious 3-rule decomposition was
  rejected by the conflict matrix because the rules read each other's
  writes.
- A round-robin arbiter whose every-cycle behaviour is "scan, grant, update
  state." The `pilot_round_robin_arbiter.rs` is single-rule for the same
  reason.

Multi-rule decomposition shines when **at most one of several alternatives
fires per cycle**: state-machine transitions where the priority chain
elects the winning transition, command processors with mutually exclusive
command types, scheduler-arbitrated multi-port writes. The
`pilot_simple_uart_tx.rs` example uses 3 mutually-exclusive rules
(`load` / `advance` / `finish`) — each writes the same bit-counter, the
guards are pairwise unsatisfiable, the priority chain elides redundant
suppressors via `mutually_exclusive`. That's a multi-rule kernel earning
its keep.

When porting a BSV design, check each `module mkFoo` against this question
before reaching for `rule_kernel!`. A pure dataflow widget (CRC, MAC, FIR
filter, encoder/decoder) may not benefit from rules at all and is better
written as a regular RHDL widget.

---

## 9 — Worked example: round-robin arbiter

This is the simplest non-trivial worked port. The full BSV version is on
the left; the RHDL port (which is the actual `pilot_round_robin_arbiter.rs`
test in the repository) is on the right.

### BSV

```bsv
module mkRoundRobinArbiter#(Bit#(N) requests, Bit#(W) ifc);
   Reg#(Bit#(W)) last_granted <- mkReg(0);
   Reg#(Bool)    valid        <- mkReg(False);

   rule arbitrate;
      Bit#(W) start = valid ? last_granted + 1 : 0;
      Bit#(W) winner_idx = 0;
      Bool    found      = False;

      for (Integer i = 0; i < N; i = i + 1) begin
         Bit#(W) idx = start + fromInteger(i);
         if (requests[idx] == 1 && !found) begin
            winner_idx = idx;
            found      = True;
         end
      end

      last_granted <= found ? winner_idx : last_granted;
      valid        <= found;
   endrule

   method Maybe#(Bit#(W)) grant;
      …  // same scan, returns tagged Valid winner_idx if found.
   endmethod
endmodule
```

### RHDL

```rust
rule_kernel! {
    pub struct RuleRoundRobinArbiter<const N: usize, const W: usize>
    where
        rhdl::bits::W<N>: BitWidth,
        rhdl::bits::W<W>: BitWidth,
    {
        last_granted: dff::DFF<Bits<W>>,
        valid:        dff::DFF<bool>,
    }

    impl RuleRoundRobinArbiter {
        #[rule]
        fn arbitrate(ctx: &mut RuleCtx<Self>, requests: Bits<N>) {
            let start: Bits<W> = if *ctx.valid {
                *ctx.last_granted + bits::<W>(1)
            } else {
                bits::<W>(0)
            };
            let mut winner_idx: Bits<W> = bits::<W>(0);
            let mut found = false;
            for i in 0..N {
                let offset: Bits<W> = bits::<W>(i as u128);
                let idx: Bits<W>    = start + offset;
                let bit_at_idx      = (requests >> idx) & bits::<N>(1);
                if bit_at_idx != bits::<N>(0) && !found {
                    winner_idx = idx;
                    found      = true;
                }
            }
            ctx.last_granted = if found { winner_idx } else { *ctx.last_granted };
            ctx.valid        = found;
        }

        #[output]
        fn output(self_q: &Self, requests: Bits<N>) -> Option<Bits<W>> {
            // Same scan; returns Some(winner_idx) when found.
            …
        }
    }
}
```

### What survived the port

- **Atomic non-blocking semantics** — `<=` ⇄ `ctx.field =`.
- **Combinational `let` for intermediate values** — same idiom in both
  languages; RHDL hoists into the per-rule preamble.
- **Generic over data widths** — `Bit#(N)` ⇄ `Bits<N>` with the
  `rhdl::bits::W<N>: BitWidth` constraint.
- **The scan loop body** — almost identical. Rust's `bits::<W>(i as u128)`
  replaces `fromInteger(i)`, and the if-condition uses `&&` plus `!found`.

### What's different

- **Output is in a separate `#[output]` method**, not inline as a `method
  grant`. BSV's interface methods become RHDL's value-method.
- **Generics use Rust syntax** including the `where` clause for `BitWidth`
  bounds.
- **Tests live in Rust**, with `#[test]` and the framework's `run` /
  `synchronous_sample` combinators rather than BSV's testbench module.

For larger worked examples (a small RISC-V pipeline, a cache controller),
see [the upstream design plans referenced from `rule-architecture.md` §3].
Those are deferred to follow-up chapters once the rule-kernel surface is
exercised against more substantial designs.

---

## 10 — Things RHDL has that BSV doesn't

For completeness, the differentiators that motivate the move:

- **Phantom-typed clock domains.** `Signal<T, Red>` cannot be assigned to
  `Signal<T, Blue>` without an explicit `cdc::*` widget. Compile error if
  you try. BSV's clock-domain story is implicit and easy to violate.
- **Rust's tooling.** `cargo`, `rustdoc`, `clippy`, `rust-analyzer`,
  `cargo doc`, and (eventually) the package manager described in
  `package-manager-architecture.md`.
- **Generics over types and const-bit-widths.** RHDL's generics are Rust's
  generics. No proprietary type system to learn.
- **No proprietary compiler.** Open source, contributable, debuggable from
  the ground up.

---

## 11 — Things BSV has that RHDL v1 doesn't

Honesty section. The gaps:

- **Methods (modular rules).** BSV's interface methods can be called from
  other modules with their own scheduling implications. v1 of `rhdl-rule`
  does not have this — see `rule-architecture.md` §17.1 (v1 non-goal).
  Workaround for now: explicit `RCStream` handshakes or hand-written
  `Synchronous` wrapper widgets.
- **Cross-module scheduling.** BSV can schedule rules from two distinct
  modules together. v1 schedules each `RuleKernel` independently.
- **Maximal parallel firing.** v1 of `rhdl-rule` ships strict-priority
  arbitration; maximal parallel firing is Phase 3 of the design plan.
- **Cross-clock rules.** Rules in v1 are intra-domain; cross-domain
  communication uses the existing `cdc::*` widgets.

Each of these is a deliberate v1 scope-cut, not a permanent limitation.
The path to closing them is documented in `rule-architecture.md` §16
(phasing).

---

## 12 — Diagnostics shipped for BSV-fan ergonomics

Per `rule-architecture.md` §17.4 play 2 ("beat BSV on rule-scheduler
diagnostics" — the strategic wedge), these compile-time errors and
warnings are shipped:

| Error / warning | Triggered by | Diagnostic |
|---|---|---|
| `conflict_free` violation | Asserting two rules conflict-free when the read/write sets actually overlap | Compile error pointing at the asserting rule, naming the offending field. |
| `urgent_before` cycle | A and B both have `urgent_before` edges to each other (transitively) | Compile error pointing at one rule on the cycle. |
| `urgent_before` self-loop | Rule references itself in `urgent_before` | Compile error. |
| `urgent_before` unknown rule | Annotation references a rule that doesn't exist | Compile error with the bad name. |
| `urgent_before` meaningless | Annotation between rules with no shared read/write set | Compile error — there's nothing to schedule. |

Deferred (tracked in CHANGELOG follow-ups): write-read suppression visible
as a compile-time NOTE; suggested-annotation hints at the call site;
conflict-graph visualization in errors.

---

## References

- `rule-architecture.md` (repository root) — the full design plan; this
  chapter assumes you've at least skimmed it.
- `pilot_round_robin_arbiter.rs` — the worked-example port shown in §9.
- `pilot_fifo_write_logic.rs` — the "single-rule is right" example referenced
  in §8.
- `pilot_simple_uart_tx.rs` — the multi-rule state-machine example
  referenced in §8.
- `pilot_composition.rs` — the rule-kernel-plus-traditional-widget
  composition example referenced in §7.
- `direct_assignment.rs` — the canonical test for the direct-assignment +
  per-rule preamble syntax used throughout this chapter.

[^bluespec]: Arvind, R.S. Nikhil. *Bluespec System Verilog: Efficient,
Correct RTL from High-Level Specifications.* MEMOCODE 2004. The canonical
Bluespec reference; the rule-and-scheduler model RHDL adapts.
