# Rule Architecture for RHDL — Design Plan

A proposal for **rhdl-rule**: Bluespec-style guarded atomic rules as a first-class extension to RHDL. Rules are declarative concurrent specifications — each rule names a guard predicate and a set of state updates, with the compiler responsible for analyzing inter-rule conflicts and synthesizing a deterministic scheduler that fires a maximal non-conflicting subset of ready rules each cycle. The result is a regular RHDL `Synchronous` widget; rules are *sugar*, not a parallel runtime.

This is the sixth compiler-and-language design plan, alongside `auto-pipelining-plan.md`, `kernel-language-extensions.md`, `vendor-primitive-architecture.md`, `fsm-architecture.md`, and `stream-bus-architecture.md`. Like those, it is independently shippable in phases. It interlocks tightly with `fsm-architecture.md` (rules subsume FSMs in many cases — the rule scheduler *is* the next-state computation), `stream-bus-architecture.md` (rules naturally produce/consume `RCStream` items), and `kernel-language-extensions.md` (rule bodies use the kernel-accepted Rust subset).

The design is grounded in the user-supplied proposal `rhdl_rule_full_proposal.md` (this document expands every section of that proposal with concrete syntax, lowering, validation, and phasing).

The thesis: Bluespec proved that guarded atomic rules eliminate entire classes of FSM bugs by moving scheduling from the user to the compiler. RHDL has the type system and IR layering to do the same thing inside a Rust-embedded language with cleaner clock-domain typing than Bluespec ever had. Rules become RHDL's answer to "I want to express the *what* and let the compiler figure out the *when*."

---

## 1 — Motivation

Modern RTL design suffers from four chronic problems: (1) **manual scheduling** of when state updates happen, with the designer hand-encoding the priority of competing updates as nested `if-else` chains; (2) **implicit race conditions** when the same register is updated from multiple paths; (3) **FSM explosion** as designers manually flatten concurrent processes into a single state enum; (4) **poor composability** because two correctly-designed FSMs can deadlock or thrash when wired together if their interactions weren't anticipated.

Bluespec System Verilog (BSV) demonstrated that **guarded atomic rules + compiler-synthesized scheduling** eliminate all four. The user writes rules as independent guarded actions; the compiler proves which can fire concurrently, generates an arbitration network, and emits the same RTL the designer would have written by hand — but provably free of the four hazards above. Bluespec has shipped silicon at large scale (XLNX X670, Charles River, Bluespec partners) and academic ASICs (RISC-V Riscy-OO, BSV-targeted compilers) for two decades.

RHDL today has the structural ingredients for guarded atomic rules but lacks the surface syntax and the compiler-synthesized scheduler. The kernel-as-pure-fn invariant — the same property that makes auto-pipelining sound and FSM verification tractable — is *exactly* what a rule scheduler needs. A Bluespec-style scheduler is a compile-time program transformation; it has no runtime. It produces a regular synchronous circuit. RHDL's IR and macro layers can do this with no new fundamental hardware semantics.

This plan adopts Bluespec's *semantic model* (guarded atomic rules, conflict-driven scheduling, atomic commit) while replacing Bluespec's *language* with Rust-embedded RHDL. The result is the strongest combination on offer: Bluespec's correctness story, RHDL's clock-domain typing, Rust's tooling.

---

## 2 — Design goals (and explicit non-goals)

**Goals.**

- Preserve RHDL's clock-domain typing — every `Reg<T, Clk>` carries its clock domain in the type system.
- Generate standard RHDL `Synchronous` kernels — rules are sugar, not a new runtime.
- No runtime scheduling, no interpretation — all conflict analysis and scheduler synthesis happens at compile time.
- Deterministic compilation — given the same rule set with the same priority annotations, the compiler produces byte-identical Verilog every time.
- Atomic rule semantics — every rule either fires completely or not at all in a given cycle. Partial firing is impossible by construction.
- Composable with the rest of RHDL — a `RuleKernel` widget drops into any place a `Synchronous` widget does. Existing widgets compose with rule kernels without modification.

**Non-goals (v1 explicitly out of scope).**

- Cross-clock rules — rules are confined to a single clock domain. Cross-domain communication uses `cdc::*` widgets (Sync1Bit, async FIFO, slow crosser) per the existing pattern.
- Full Bluespec method system — Bluespec has methods (rule fragments callable from other modules) with their own complex scheduling implications. v1 ships rules without methods.
- Global scheduling across module boundaries — Bluespec can schedule rules from two distinct modules together. v1 schedules each `RuleKernel` independently.
- Optimal parallel firing — Phase 1 ships a priority-based scheduler (one rule per cycle in worst case for conflicting rules). Maximal-parallel firing is Phase 3.

---

## 3 — Where this sits

The design plan family now has six documents covering distinct concerns. Rule kernels interlock with all of them:

| Plan | Relationship to rules |
|---|---|
| `fsm-architecture.md` | A rule kernel that wraps a single state register *is* an FSM, and its rule scheduler *is* the next-state function. Rules subsume FSMs in many cases; FSM derive subsumes rules in pure-state-machine cases. The two compose: a `Reg<State>` where `State: Fsm` makes both the rule scheduler and the FSM static analysis available. |
| `kernel-language-extensions.md` | Rule bodies use the kernel-accepted Rust subset. Most extensions in the kernel-language spec (or-patterns, range patterns, guards, `?`) work inside rule bodies once shipped. |
| `auto-pipelining-plan.md` | The synthesized scheduler and the synthesized next-state mux are normal NTL combinational logic that the auto-pipeliner can retime/insert registers across. No special integration required. |
| `vendor-primitive-architecture.md` | Orthogonal at the rule level, but a rule whose action invokes a vendor primitive (e.g. a DSP-MAC) lowers normally — the vendor-primitive request goes into the rule's lowered next-state computation. |
| `stream-bus-architecture.md` | Rules naturally produce and consume `RCStream<T, F, D>` items. A rule whose action writes to an `RCStream`'s data slot is a clean producer pattern; a rule whose guard predicates on `data.is_some()` is a clean consumer. The interlock makes rule-kernel-driven stream pipelines natural. |

The new track is independently shippable; none of the others block it.

---

## 4 — Concrete syntax

The proposal sketches a sparse syntax. This section makes it concrete.

### 4.1 The widget shape

```rust
use rhdl::prelude::*;
use rhdl_rule::prelude::*;  // adds Reg, RuleKernel, RuleCtx, guard!, set!

#[derive(Clone, Debug, RuleKernel)]
#[rhdl(dq_no_prefix)]
pub struct CounterAndFlag {
    counter: Reg<b8>,
    flag:    Reg<bool>,
}

impl CounterAndFlag {
    #[rule(priority = 0)]
    fn increment(ctx: &mut RuleCtx<Self>, i: In) {
        guard!(*ctx.flag);
        guard!(i.enable);
        set!(ctx.counter, *ctx.counter + 1);
    }

    #[rule(priority = 1)]
    fn reset_on_max(ctx: &mut RuleCtx<Self>) {
        guard!(*ctx.counter == 255);
        set!(ctx.counter, 0);
        set!(ctx.flag, false);
    }

    #[rule(priority = 2)]
    fn raise_flag(ctx: &mut RuleCtx<Self>, i: In) {
        guard!(i.start);
        guard!(!*ctx.flag);
        set!(ctx.flag, true);
    }
}
```

**Decoding.** The `#[derive(RuleKernel)]` macro:

1. Identifies the struct's `Reg<T>` fields. Each becomes a state register with a typed `RuleCtx<Self>` accessor.
2. Walks all `impl Self` methods marked `#[rule]`. Each becomes a rule.
3. Statically analyzes each rule's body to determine its read-set, write-set, and guard expression.
4. Builds the conflict matrix between rules.
5. Synthesizes the scheduler.
6. Emits a regular RHDL `Synchronous` widget whose kernel implements the per-cycle "run all guards → resolve conflicts → fire winning rules → commit next state" semantics.

The `RuleCtx<Self>` is a generated type that exposes each `Reg<T>` field as a typed slot supporting `*ctx.field` (read) and `set!(ctx.field, value)` (write). Reads are tracked statically (any `*ctx.field` or method call on `ctx.field` adds to the rule's read-set); writes are syntactically marked by `set!`.

### 4.2 Rule-body vocabulary

Inside a `#[rule]` method body the macro recognises three constructs:

- **`guard!(expr)`** — append a guard. The rule fires only if every guard's expression evaluates to true. Multiple `guard!` calls in a rule body are conjoined.
- **`ctx.field = value;`** (canonical) — schedule a write to a register. Read as: "at the next clock edge, this register takes `value`." Despite the surface syntax, the assignment is **non-blocking** by construction: every `ctx.field = …;` in a rule body commits atomically at the cycle boundary, after every other rule's writes have been computed against the same pre-firing snapshot. Multiple direct assignments in one rule body all fire together when the rule fires.
- **`set!(ctx.field, value)`** (legacy) — the macro spelling that predated direct assignment. Identical semantics; kept for backward compatibility. New code should prefer the direct-assignment form.
- **`*ctx.field`** — read a register (sugar for `Deref::deref`). The macro tracks this as a read-set entry.

Any other statements in a rule body — typically `let` bindings — are preserved as a **per-rule preamble**: they execute once per cycle and are in scope for every action expression in the same rule. This means a rule can compute intermediate values once and reference them in multiple writes:

```rust
#[rule]
fn step(ctx: &mut RuleCtx<Self>, i: PreambleFifoIn<N>) {
    // Preamble: shared computation visible to all actions.
    let full       = (*ctx.write_address + bits::<N>(1)) == i.read_address;
    let will_write = i.write_enable && !full;

    // Three action assignments referencing the preamble.
    ctx.write_address         = if will_write { *ctx.write_address + bits::<N>(1) } else { *ctx.write_address };
    ctx.overflow              = *ctx.overflow || (i.write_enable && full);
    ctx.write_address_delayed = *ctx.write_address;
}
```

Without the preamble, the user would have to inline `(*ctx.write_address + bits::<N>(1)) == i.read_address` in three places (the legacy limitation called out in `pilot_fifo_write_logic.rs` Move-1 retrospective).

#### Why `=` and not `<=`

BSV's non-blocking register write is spelled `<=` (and BSV's `=` is reserved for combinational `let`). RHDL uses `=` for register writes inside rule bodies because:

- `<=` in Rust is the comparison operator. Overloading it inside a macro to mean "non-blocking write" would surprise readers — they'd see what looks like a Boolean comparison and have to know it's actually a hardware action.
- The atomicity is guaranteed by the **scope** the assignment appears in (a `#[rule]` method body), not by the **operator**. Inside a rule body every `=` is non-blocking by definition; outside, it's a normal Rust assignment.
- The `RuleCtx<Self>` phantom type around `ctx` makes it impossible to *actually* mutate anything at runtime — the field accessors are inert markers; the macro is the only thing giving them meaning. Readers who recognize the phantom-type pattern see immediately that `ctx.field = value` is metadata, not mutation.

For BSV users, the mental translation is **`reg <= value;` (BSV) ⇄ `ctx.reg = value;` (RHDL)** with the operator change explained in the BSV→RHDL porting guide (§17.4 play 3). Both have the same semantics: non-blocking, atomic, fires when the rule fires.

The `set!` macro stays available for users who prefer the explicit-keyword spelling; it produces byte-identical hardware.

### 4.3 Annotations

Rule-level attributes carried in `#[rule(...)]`:

- `priority = N` — explicit priority. Lower numbers fire first when conflicts arise. Default is the source-code order of the rule definition.
- `urgent_before("rule_name")` — explicit ordering against another named rule (Bluespec compatibility).
- `conflict_free("rule_name")` — assertion that two rules never conflict. Compiler verifies; if false, a compile error.
- `mutually_exclusive` — applied to a group of rules to assert that at most one of them is ever ready in a cycle (their guards are pairwise unsatisfiable simultaneously). Compiler can use this to optimize the scheduler.
- `trace` (or `trace = true` / `trace = false`) — opt-in per-rule trace exposure. When set, the macro emits additional `let fire_<rule>` and `let can_fire_<rule>` bindings (visible names — no underscore prefix) so the rule's firing decisions show up in VCDs. Off by default: typical kernels don't pay the kernel-emission and VCD-clutter cost for debug-only signals. Enable selectively on the rules you need to observe; it composes freely with the other annotations.

Phase 2 ships annotations beyond `priority`. Phase 1 ships only `priority` plus implicit source-order priority.

### 4.4 Inputs and outputs

A `RuleKernel` has the same I/O contract as a `Synchronous` widget: an `In` input type and an `Out` output type.

- Inputs are read-only slots in `RuleCtx`. Rules can read them; rules cannot write them.
- Outputs are *derived* from the post-firing state. The widget specifies an `output` method or a designated `#[output]` rule that computes the output combinationally from the post-firing state and the input. Bluespec calls this a "value method"; we call it the output kernel.

```rust
impl CounterAndFlag {
    #[output]
    fn output(&self, _i: In) -> Out {
        Out { count: *self.counter, ready: *self.flag }
    }
}
```

The `#[output]` method runs at the end of every cycle, after all rules fire and the next-state has been computed. It's a pure function of the (post-firing) state and the (current cycle's) input.

### 4.5 Two surface forms: function-like `rule_kernel!` and `#[rule_kernel_attr]`

`rhdl-rule` ships **two equivalent spellings** for the same lowering. Both share the same internal `lower_rule_kernel` function in `rhdl-rule-core`; a token-level parity test (`attribute_form_parity.rs`) keeps them honest. Pick the spelling that reads more naturally for your widget.

#### Function-like form

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
        fn bump(ctx: &mut RuleCtx<Self>, enable: bool) {
            guard!(enable);
            set!(ctx.count, *ctx.count + bits::<8>(1));
        }

        #[output]
        fn output(self_q: &Self, _enable: bool) -> Bits<8> {
            *self_q.count
        }
    }
}
```

The macro receives the struct + impl in one invocation, auto-injects the standard derives (`Synchronous`, `SynchronousDQ`, `Default`), and emits the lowered kernel.

#### Attribute-on-impl form

```rust
use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel_attr as rule_kernel;

#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct Counter {
    count: dff::DFF<Bits<8>>,
}

#[rule_kernel]
impl Counter {
    #[rule]
    fn bump(ctx: &mut RuleCtx<Self>, enable: bool) {
        guard!(enable);
        set!(ctx.count, *ctx.count + bits::<8>(1));
    }

    #[output]
    fn output(self_q: &Self, _enable: bool) -> Bits<8> {
        *self_q.count
    }
}
```

The attribute receives only the impl block. The user writes the standard RHDL derives on the struct themselves, exactly like every other RHDL widget. This shape mirrors the `#[derive(Synchronous, SynchronousDQ)] + #[kernel] fn` convention used by hand-written widgets — same coordination model: the macros emit independent code that the type system links together.

#### How they coexist

This works because rule kernels never needed cross-macro *state*; they needed cross-macro *layout convention*. The function-like form gets that convention by seeing both items in one token stream. The attribute form gets it the same way `#[derive(Synchronous)] + #[kernel]` already do today: each macro emits standalone code; `SynchronousIO`'s `type Kernel = ...` line is the linkage; trait resolution is the rendezvous.

Concretely:
- The function-like form's macro analyses the struct + impl together and emits both.
- The attribute form's macro analyses the impl alone, derives field names from the union of every rule's read/write set + the output method's field reads, and emits the `SynchronousIO` impl + the `#[kernel]` function. The struct (with its derives) is the user's responsibility.
- `lower_rule_kernel` in `rhdl-rule-core` is the single source of truth for both. Pre-PR refactor: the function-like form was the only entry point; the attribute form is a ~20-line wrapper that calls the same inner function.

#### Auto-hold for unused struct fields (function-like form only)

The function-like form passes the struct's actual field list to the lowering. Any field that no rule reads or writes and that the `#[output]` method doesn't reference gets **auto-hold semantics**: the lowered kernel emits `_next_<field> = q.<field>` and no rule ever overwrites it, so the field stays at its current value forever. The user can declare DFF fields without being forced to add `let _ = *self_q.x;` workarounds in the output method just to satisfy the macro.

The attribute form can't see the struct, so it has no field list to compare against — unused fields will surface as Rust compile errors ("missing field `xyz` in initializer of D"). Three options for users of the attribute form:
1. Touch the field in some rule (`*ctx.field` read or `ctx.field = ...` write) — explicit auto-hold equivalent.
2. Reference the field in the `#[output]` method.
3. Switch the widget to the function-like form for auto-hold.

This asymmetry is the price the attribute form pays for not seeing the struct definition. The diagnostic surface is on the roadmap (a clear miette-style "field `xyz` is missing" message that names the field and suggests the three fixes).

#### `#[output]` without `self_q` when state isn't read

The `#[output]` method accepts two signatures:

```rust
// Form A — receiver + input.  Use when the body reads state via
// `*self_q.field` or `self_q.field`.
#[output]
fn output(self_q: &Self, i: I) -> O { ... }

// Form B — input only.  Use when the body is purely a function of
// the input (no state read).  No need to declare or silence an
// unused `self_q` parameter.
#[output]
fn output(i: I) -> O { ... }
```

The macro detects which form by counting parameters and checking the first parameter's shape: a receiver (`&self`) or a typed reference (`self_q: &Self`) signals Form A; otherwise it's Form B with the parameter as the input. If the body of a Form B method later needs to read state, switch to Form A and the `*self_q.field` rewriting kicks in. The two forms are otherwise identical in everything they emit — Form B just drops the dead parameter.

#### When each form reads better

| Situation | Recommended form |
|---|---|
| Toy / single-widget tests | function-like (one invocation, fewer lines) |
| Widget alongside hand-written widgets in the same crate | attribute (matches the surrounding `#[derive(Synchronous, SynchronousDQ)] + #[kernel] fn` shape) |
| User wants explicit control over which derives are applied | attribute (no auto-injection) |
| User wants minimum boilerplate | function-like |
| Rule kernel needs custom derives or attributes on the struct | attribute (the function-like form auto-injects, which can clash) |

#### What's left to ship

A pure `#[derive(RuleKernel)]` (no attribute on the impl) would also be implementable — the derive would emit standard `Synchronous`/`SynchronousDQ`-equivalent impls and a marker trait, and would coordinate with a separate `#[rule_kernel_attr]` on the impl via the same trait-resolution model. We don't ship that today because:
1. It requires re-implementing what `#[derive(Synchronous, SynchronousDQ)]` already do, which means depending on `rhdl-core`'s codegen internals — a structural change `rhdl-rule-core` is not currently allowed by `architecture.md`.
2. The attribute form is one extra `#[derive(...)]` line away from the §4.1 sketch — modest ergonomic delta.

If we later decide it's worth it, the path is: extract the `Synchronous`/`SynchronousDQ` codegen into a published library crate that both `rhdl-macro-core` and `rhdl-rule-core` can depend on, then make `#[derive(RuleKernel)]` re-emit those impls.

#### Note on the §4.1 sketch

The `#[derive(RuleKernel)]` shape in §4.1 remains the **aspirational surface** the design plan was originally drafted around. The attribute form shipped here is the *practical equivalent* — it requires one extra `#[derive(...)]` line on the struct (which the user would have written anyway for any other RHDL widget) and gets all the same hardware behavior. Treat §4.1 as showing the spirit of the API; treat the function-like and attribute forms shown above as the two literal spellings the compiler accepts today.

---

## 5 — Execution semantics

Each clock cycle, the synthesized scheduler executes:

```
1.  Evaluate all rule guards against the current (pre-firing) state and input.
2.  Compute can_fire_i = guard_i for each rule i.
3.  Resolve conflicts via the priority-arbitrated scheduler:
    fire_i = can_fire_i AND NOT (any higher-priority rule j conflicts with i AND fire_j).
4.  For each register, compute its next-state value as the action of the firing rule
    that writes it (priority ensures at most one firing rule writes any register).
5.  At the next clock edge, atomically commit all next-state values.
```

The result is **observationally equivalent** to a sequential execution of the firing rules in priority order. This is Bluespec's fundamental theorem applied to RHDL.

Atomicity falls out of the lowering: every register's next-state value is computed as a single combinational expression before being clocked. There is no intermediate state. A rule's writes either all happen or none of them happen; there is no path by which a rule's `set!` to register A succeeds while its `set!` to register B fails.

---

## 6 — Conflict model

### 6.1 Definition

Two rules conflict if they have any of the following overlap:

| Case | Conflict? |
|---|---|
| `write_set(a) ∩ write_set(b) ≠ ∅` | yes |
| `write_set(a) ∩ read_set(b) ≠ ∅` | yes |
| `read_set(a) ∩ write_set(b) ≠ ∅` | yes |
| `read_set(a) ∩ read_set(b) ≠ ∅` and no other overlap | no |

Read-read does not conflict because both rules see the same pre-firing value of the shared register.

### 6.2 Conflict matrix

For *N* rules, the conflict matrix is *N × N* with `[i, j] = (i, j conflict per the table above)`. Symmetric for write-write conflicts; relevant for asymmetric scheduling decisions in read-write conflicts.

The matrix is built at compile time by the macro layer's read/write extraction step.

### 6.3 Resolution

When two conflicting rules are both ready in a cycle, exactly one fires:

- **Phase 1**: priority. The lower-priority rule fires; the higher-priority rule does not. (Numerically lower `priority = N` wins; `priority = 0` is highest priority.)
- **Phase 2**: priority + explicit `urgent_before` annotations.
- **Phase 3**: priority + annotations + maximal-parallel-firing optimization (non-conflicting rules fire concurrently; only the priority chain matters for true conflicts).

---

## 7 — Scheduler synthesis

The scheduler is a combinational circuit producing one `fire` signal per rule. For *N* rules ordered by priority:

```
fire_0 = can_fire_0
fire_1 = can_fire_1 AND NOT (conflict[0][1] AND fire_0)
fire_2 = can_fire_2 AND NOT (conflict[0][2] AND fire_0)
                    AND NOT (conflict[1][2] AND fire_1)
...
fire_i = can_fire_i AND NOT OR_{j < i, conflict[j][i]} (fire_j)
```

The chain has length *N* in the worst case. For typical rule sets (10–30 rules), this is a few levels of logic — well within combinational-path budgets.

For Phase 3 (parallel firing), the scheduler is restructured: rules with empty conflict-set rows are removed from the priority chain entirely (they always fire when ready). Rules with sparse conflicts get pairwise mutex gating instead of a full priority chain. The transformation is an optimization pass over the conflict matrix.

---

## 8 — Lowering to RHDL

The output of the rule-kernel macro is a regular `#[derive(Synchronous, SynchronousDQ)]` widget plus a `#[kernel]` function. The kernel is generated from the rules.

### 8.1 Generated structure

```rust
// User wrote:
#[derive(RuleKernel)]
pub struct CounterAndFlag {
    counter: Reg<b8>,
    flag: Reg<bool>,
}
impl CounterAndFlag { /* rules, output */ }

// Macro generates approximately:
#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
pub struct CounterAndFlag {
    counter: dff::DFF<b8>,
    flag:    dff::DFF<bool>,
}

impl SynchronousIO for CounterAndFlag {
    type I = In;
    type O = Out;
    type Kernel = counter_and_flag_kernel;
}

#[kernel]
pub fn counter_and_flag_kernel(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    // Compute can_fire signals.
    let can_fire_increment    = q.flag && i.enable;
    let can_fire_reset_on_max = q.counter == 255;
    let can_fire_raise_flag   = i.start && !q.flag;

    // Resolve conflicts via priority chain.
    let fire_increment    = can_fire_increment;
    let fire_reset_on_max = can_fire_reset_on_max && !(/* conflict[0][1] AND fire_0 */ ...);
    let fire_raise_flag   = can_fire_raise_flag   && !(/* ... */ ...);

    // Compute next-state for each register.
    let next_counter = if fire_reset_on_max { 0 }
                       else if fire_increment { q.counter + 1 }
                       else { q.counter };
    let next_flag    = if fire_reset_on_max { false }
                       else if fire_raise_flag { true }
                       else { q.flag };

    // Output kernel.
    let out = Out { count: q.counter, ready: q.flag };

    // Reset (last, per CLAUDE.md convention).
    if cr.reset.any() {
        return (Out::default(), D { counter: 0, flag: false });
    }
    (out, D { counter: next_counter, flag: next_flag })
}
```

The generated kernel respects every CLAUDE.md convention: derives, `dq_no_prefix`, reset-comes-last, single combinational expression for next-state. The downstream RHDL compiler (RHIF → RTL → NTL → Verilog) does not know rules existed — it just sees a normal synchronous kernel. Auto-pipelining, FSM analysis, and every other downstream pass operates on the lowered form unchanged.

### 8.2 What the macro layer must extract

For each `#[rule]` method:

- **Read set**: every `*ctx.field`, `ctx.field.deref()`, and method call on `ctx.field` that observes a state register.
- **Write set**: every `set!(ctx.field, ...)` invocation.
- **Guard expression**: the conjunction of all `guard!(...)` calls in the rule body. Implicit `true` if no guards are present.
- **Action**: the post-guard rule body, expressed as a function from `(state, input)` to `(updates: WriteSet)`.

The macro's static analysis is a small AST walk; it lives in `rhdl-macro-core/src/rule.rs`.

---

## 9 — Composability

### 9.1 With existing widgets

A `RuleKernel` widget is structurally a `Synchronous` widget. It composes with every other RHDL widget by virtue of implementing `Synchronous` + `SynchronousIO` + `SynchronousDQ` (the latter auto-derived). A parent widget can hold a `RuleKernel` as a field; a `RuleKernel` can hold non-rule sub-widgets (FIFOs, counters, RAM blocks) as auxiliary fields not subject to rule semantics.

### 9.2 With FSM derive

A rule kernel whose state includes a `Reg<State>` where `State: Fsm` is a perfect storm of the two design plans. The rule scheduler synthesizes the FSM's transition function; the FSM derive provides the metadata (variant names, default variant) for the static-analysis passes (reachability, dead-state, formal verification). A widget written this way gets:

- The rule scheduler's atomic-update guarantee (no intra-cycle races).
- The FSM derive's reachability + dead-state diagnostics (Layer 2 of `fsm-architecture.md`).
- The FSM derive's auto-generated state diagrams in rustdoc (Layer 3).
- The FSM derive's formal-verification surface (Layer 4 / 5).

This is a stronger combination than either design plan offers alone. It is the recommended pattern for any FSM-shaped widget that has more than two or three rules.

### 9.3 With RCStream

Rules naturally produce and consume `RCStream<T, F, D>` items. A producer rule:

```rust
#[rule]
fn produce(ctx: &mut RuleCtx<Self>, i: In) {
    guard!(*ctx.tx_ready);  // downstream RCStream is ready
    guard!(/* item is available */);
    set!(ctx.tx_data, Some(Item { data: ..., frame: ... }));
}
```

A consumer rule:

```rust
#[rule]
fn consume(ctx: &mut RuleCtx<Self>, i: In) {
    guard!(i.rx_data.is_some());
    let item = i.rx_data.unwrap();  // safe by guard
    set!(ctx.something, item.data);
    set!(ctx.ready_to_consume, false);
}
```

The compositional story is clean: rules have natural backpressure semantics that match `RCStream`'s ready/valid handshake. The Carloni LID theorem guarantees that inserting `RCStreamRelay` between two rule-kernel widgets does not change observable behavior.

### 9.4 With kernel-language-extensions

Rule bodies use the kernel-accepted Rust subset. Once `kernel-language-extensions.md` Tier 1 ships (or-patterns, range patterns, guards, `@` bindings, `?`), all of those work inside rule bodies for free. The `set!` and `guard!` macros are simply a thin layer over the kernel-language constructs.

### 9.5 With auto-pipelining

The synthesized scheduler is a combinational network. The synthesized next-state mux per register is also combinational. Both are visible to the auto-pipeliner as ordinary NTL nodes. If a rule kernel has a long combinational path through the scheduler (say, 50 rules with extensive conflict overlap), the auto-pipeliner can insert pipeline registers — though in practice the scheduler depth grows logarithmically with rule count for sparse conflict matrices.

---

## 10 — Clock domain model

Per the proposal, **all signals in a `RuleKernel<Clk>` must live in the same clock domain**. This is enforced at the type level:

- Every `Reg<T>` is implicitly `Reg<T, Clk>` where `Clk` is the kernel's clock domain.
- Inputs in `In` must all be `Signal<T, Clk>` (or unwrapped by the framework's implicit clock-reset fan-out).
- Cross-domain references are compile errors: `Reg<T, ClkA>` used in a `RuleKernel<ClkB>` body fails to typecheck.

Cross-domain communication uses the existing `cdc::*` widgets. A rule kernel that needs data from another domain consumes it via an `AsyncFifo<T, ClkA, ClkB>`, a `Sync1Bit<T, ClkA, ClkB>`, or a `SlowCrosser<T, ClkA, ClkB>`. The CDC widget itself is *not* a rule kernel; it's a normal RHDL `Circuit` (per the existing async-circuit category).

This is the right cut: rule semantics are intrinsically intra-domain (atomicity within a clock); cross-domain semantics need the established CDC primitives. Phase 1 enforces single-domain; multi-domain rule scheduling (Bluespec calls it "synchronizers") is out of scope for this design plan.

---

## 11 — Reset semantics

The widget's reset signal disables all rule firing for the cycle and resets every `Reg<T>` to its default value. Specifically:

- During reset (`cr.reset.any()`), every `fire_i` is forced low.
- Every `Reg<T>::default()` is committed at the next clock edge.
- The output kernel runs against the post-reset state, producing whatever output the kernel computes for the default state.

This integrates cleanly with the existing CLAUDE.md convention of "reset comes last" — the macro's generated kernel places the reset block at the end of the generated kernel function.

---

## 12 — Error handling and diagnostics

Every diagnostic surface emits via `miette` with source-span information. Categories:

- **Conflicting writes detected at compile time.** Two rules with no priority annotation that have a write-write conflict — the macro flags the ambiguity and asks for an explicit priority.
- **Conflict-free assertion violated.** A `#[rule(conflict_free("other"))]` annotation on a rule whose computed conflict set actually contains "other" — compile error.
- **Mutual-exclusion assertion violated.** A `#[rule(mutually_exclusive)]` group whose guards are not provably pairwise unsatisfiable — compile error.
- **Cross-domain register access.** A rule body accessing a register from a different clock domain — compile error pointing at the offending `*ctx.foreign_field`.
- **Side effects in guards.** A `guard!()` expression that calls a mutating method or invokes `set!` — compile error.
- **Unreachable rules.** A rule whose guard is provably always false — warning (the user may have intended this as a placeholder; warn but compile).

Phase 2 of the implementation focuses on diagnostic quality. Phase 1 ships with sufficient diagnostics to make the system usable but not yet polished.

---

## 13 — IR design

Per the proposal §12, the rule extension introduces two new internal IR types in `rhdl-macro-core` (not in `rhdl-core`'s RHIF/RTL/NTL — rules lower to standard RHDL kernels before reaching those):

```rust
// Rule IR (compile-time, internal to rhdl-macro-core::rule)
struct Rule {
    name:        Ident,
    priority:    u32,
    annotations: RuleAnnotations,
    guard:       syn::Expr,           // the conjoined guard expression
    write_set:   Vec<RegRef>,         // which Reg<T> fields the rule writes
    read_set:    Vec<RegRef>,         // which Reg<T> fields the rule reads
    action:      Vec<RuleAction>,     // the rule body's set!/guard! sequence
    span:        Span,                // for diagnostics
}

struct RuleAnnotations {
    urgent_before:    Vec<Ident>,
    conflict_free:    Vec<Ident>,
    mutually_exclusive_with: Vec<Ident>,
}

enum RuleAction {
    Set { target: RegRef, value: syn::Expr },
}

// Scheduler IR (synthesized from Vec<Rule>)
struct Scheduler {
    rules:           Vec<Rule>,
    conflict_matrix: BitMatrix,          // N×N
    priority_order:  Vec<usize>,         // permutation of [0..N]
}
```

The IR is internal to the proc-macro layer. The output is a TokenStream containing the generated `Synchronous` widget and `#[kernel]` function. Downstream RHDL compilation sees only the lowered form.

---

## 14 — Compilation pipeline

Per the proposal §13, the rule extension's compilation pipeline is:

```
Rust AST (impl block with #[rule] methods)
    ↓ [rhdl-macro: walk #[rule] methods]
Rule IR (Vec<Rule>)
    ↓ [rhdl-macro: extract read/write sets from rule bodies]
Annotated Rule IR
    ↓ [rhdl-macro: build conflict matrix]
Scheduler IR
    ↓ [rhdl-macro: synthesize fire-signal expressions]
TokenStream (Synchronous widget + #[kernel] function)
    ↓ [rustc]
Standard RHDL widget
    ↓ [rhdl compiler: AST → MIR → RHIF → RTL → NTL → Verilog]
Synthesizable RTL
```

The new compiler work is the macro layer's read/write extraction and scheduler synthesis. The downstream pipeline is unchanged.

---

## 15 — Validation

Per CLAUDE.md §11.1, every phase is a compiler-level change with the full PR contract: one feature per PR, tests at every level, Justification section, documentation, CHANGELOG entry.

**Phase 1 validation matrix:**

- **Macro-expansion snapshot tests.** For each of N hand-crafted rule kernels, snapshot the generated `Synchronous` widget + kernel function. Snapshots committed to `crates/rhdl-rule/src/expect/`.
- **Functional equivalence tests.** For each rule kernel, hand-write the equivalent FSM-style kernel and verify byte-identical simulation output for a corpus of input streams.
- **Conflict-detection tests.** A library of small rule kernels with deliberate write-write, read-write, and read-read patterns; assert that the macro's reported conflict matrix matches the expected matrix.
- **Negative tests.** Cross-domain register access produces a compile error. Side-effect-in-guard produces a compile error. Conflict-free annotation violation produces a compile error.
- **End-to-end widget tests.** Rewrite three real RHDL widgets as rule kernels — `core::round_robin_arbiter`, `fifo::write_logic`, and a small protocol PHY — and verify byte-identical simulation behavior to the original implementations.
- **iverilog round-trip.** Each rule-kernel example passes `iverilog` round-trip per the standard RHDL test pattern.

**Phase 2 validation:** annotation-based optimizations are tested via before/after Verilog snapshots showing the scheduler complexity reduction.

**Phase 3 validation:** maximal-parallel-firing optimization tested by measuring the observable cycles-per-output-event metric on a corpus of rule kernels — should improve over Phase 1's strict-priority firing.

---

## 16 — Phasing

| Phase | Deliverable | Effort | Dependencies |
|---|---|---|---|
| 1 | Basic rules, conflict detection, priority scheduling, three widget rewrites | ~6 weeks | nothing |
| 2 | Annotations (`urgent_before`, `conflict_free`, `mutually_exclusive`), better diagnostics, performance | ~4 weeks | Phase 1 |
| 3 | Optimization, maximal parallel firing, partial-fire scheduling | ~6 weeks | Phase 1, 2 |

Phase 1 ships the canonical rule-kernel surface and proves the lowering on three real widgets. Phase 2 polishes the annotation system. Phase 3 is the optimization pass that brings throughput up to Bluespec parity.

---

## 17 — Comparison

### 17.1 Versus Bluespec

| Feature | Bluespec | rhdl-rule |
|---|---|---|
| Guarded atomic rules | yes | yes |
| Atomic commit | yes | yes |
| Compiler-synthesized scheduler | yes | yes |
| Conflict-free annotation | yes | yes (Phase 2) |
| Priority annotation | yes | yes (Phase 1) |
| Mutual-exclusion annotation | yes | yes (Phase 2) |
| Methods (modular rules) | yes | no (v1 non-goal) |
| Cross-module scheduling | yes | no (v1 non-goal) |
| Cross-clock rules | partial | no (v1 non-goal) |
| Clock-domain typing | no | yes |
| Embedded in mainstream language | no (own language) | yes (Rust) |
| Type system (ADTs, generics) | proprietary | Rust's |
| Tooling ecosystem | proprietary | cargo, clippy, rust-analyzer |

The biggest architectural difference: Bluespec is a standalone language with its own type system; rhdl-rule is Rust-embedded. The biggest functional advantage of Bluespec we don't replicate in v1: the method system (rules callable across module boundaries with their own scheduling implications). That's a deliberate scope-cut for v1.

### 17.2 Versus FSM derive

These are *complementary*, not competitive. FSM derive analyzes a hand-written kernel; rule derive *synthesizes* a kernel from declarative rules. A widget can use both:

- The rule scheduler produces the FSM transition function.
- The FSM derive provides reachability/dead-state analysis on the lowered transition function.
- The FSM derive provides auto-generated state diagrams.
- The FSM derive provides formal-verification properties via `#[fsm_invariant]`.

The recommended pattern for FSM-shaped widgets with multiple concurrent rules: use both derives. The user writes rules; the framework gets both the rule scheduler's atomicity guarantees and the FSM analyses' reachability/verification surface.

### 17.3 Versus existing rule-like systems in other HDLs

- **Chisel `when`/`otherwise`** — sequential sugar over muxes; no atomicity guarantees, no conflict detection, no compile-time scheduling. rhdl-rule is strictly more powerful.
- **nMigen / Amaranth `m.If/m.Elif/m.Else`** — similar to Chisel, sequential mux sugar. No rule semantics.
- **Spade `pipeline` keyword** — orthogonal; pipelining is staged, not concurrent.
- **Calyx control language** — explicit scheduling expressed as a control program. Comparable in expressiveness to rules but very different syntax.

Bluespec remains the closest comparison and the foundational reference.

### 17.4 Capturing BSV users — strategic plays in priority order

Bluespec is alive but stuck. The compiler open-sourced in 2020 (a decade after the moment had passed); the company pivoted to RISC-V IP and consulting; the truly active community is 200–500 people globally — small in headcount but disproportionately influential (senior architecture researchers at MIT/CMU/Cornell/IBM, the people writing textbooks and running graduate courses, defense-contractor architects, formal-methods-curious hardware engineers who picked BSV *specifically* because rules were worth learning a strange language for). Capturing 30–50% of them over 2–3 years is realistic and translates into outsized academic mindshare, conference papers using RHDL, course materials propagating to thousands of students per cohort, and the production validation that "the rules abstraction works under load."

The plays, in priority order:

**1. Ship `rhdl-rule` with semantics at least as strong as BSV's.** Guarded atomic rules with implicit conditions, the conflict matrix, maximal-parallel-firing schedules, all the annotation hints (`urgent_before`, `mutually_exclusive`, `conflict_free`), and methods with implicit conditions when called. Phases 1–3 of this document cover most of the surface; the implicit-condition piece on method calls (deferred per §17.1's v1 non-goal of cross-module methods) is the gap that has to close before BSV-fan engagement starts. Until it does, BSV users will see the comparison table, find "no" under "Methods (modular rules)", and disengage.

**2. Beat BSV on rule-scheduler diagnostics.** This is the wedge. Every BSV user has been frustrated by an opaque scheduling error — BSV tells them a conflict-free schedule doesn't exist but is bad at explaining *why*, because the conflict propagated transitively through a dozen rule-pair conflicts. RHDL with miette must do better: when scheduling fails, emit a diagnostic that visualizes the conflict graph, names the specific resource(s) two rules contend over, and suggests the right annotation (`urgent_before`, `mutually_exclusive`, `conflict_free`) at the right call site. §12 (Error handling and diagnostics) already commits to this direction; the bar for BSV-fan capture is "noticeable within five minutes of a BSV user trying RHDL."

**3. Publish a "BSV → RHDL" porting guide as a chapter in the RHDL book.** Side-by-side syntax. Every BSV idiom (interfaces, modules, methods with implicit conditions, `mkReg`, `mkFIFOF`, `mkConnection`, rule-with-guard, `urgent_before`, `mutually_exclusive`, `conflict_free`) gets a translation. A worked example porting a non-trivial BSV design — a small RISC-V pipeline or a cache controller — is the most credible artifact the project can produce. The chapter lives at `doc/book/src/migration/from-bsv.md`, referenced from `doc/book/src/SUMMARY.md`. **This chapter is a Phase 1 deliverable** of `rhdl-rule` per §16: the moment Phase 1 lands in main, the porting guide must be in the book.

---

## 18 — Risks and open questions

**`set!` vs `write!` naming.** Rust stdlib's `write!` macro for Display/Debug formatting clashes with the natural choice. We pick `set!` per §4.2; revisit if `set!` clashes with something we don't anticipate.

**Read-set extraction precision.** A rule body that does `let v = *ctx.field; if cond { use(v) }` reads `field` even though it might not actually need to. The macro's static analysis treats this as a read; this is conservative (over-approximates conflicts). For most patterns this is fine; in pathological cases the user can refactor to put the read inside the conditional.

**Scheduler critical path.** For *N* rules with priority chain, the scheduler is *O(N)* combinational. For *N > 50*, this can become a timing issue. Mitigations: (a) the auto-pipeliner can pipeline the scheduler; (b) Phase 3's maximal-parallel-firing optimization reduces the chain length for sparse conflict matrices; (c) the user can manually break a large rule kernel into hierarchical sub-kernels.

**Debuggability.** Rule firing patterns may be opaque in waveforms. A `#[rule]`-emitted widget should generate per-rule trace signals (one `fire_i` signal per rule, named after the rule) so the waveform viewer shows which rules fired when. This integrates with the existing `trace_*` infrastructure in `rhdl-core::trace`.

**Recursive rule dependencies.** A rule whose write-set affects another rule's guard creates a same-cycle dependency. Bluespec handles this with "urgent before" semantics: rule B's guard uses rule A's *post-firing* state if A is scheduled before B. v1 uses *pre-firing* state for all guards (conservative; equivalent to executing all rules in parallel). Phase 2 introduces `urgent_before` to allow same-cycle ordering.

**Performance scaling.** Bluespec compilations of large designs (10000+ rules in industry use) are known to be slow because of the conflict matrix computation. We need to validate that rhdl-rule's compile-time scaling is acceptable for designs in the 50–500 rule range. If it's not, we add caching or incremental compilation; this is a Phase 2 polish item.

**Determinism in macro-expansion order.** The conflict matrix construction must be deterministic across Rust compilations. We pin rule iteration order to source-code order, not hash-map iteration order. Tested via reproducible-build infrastructure.

**Composition with the existing widget library.** Some widgets in `rhdl-fpga::*` would be much simpler as rule kernels (round-robin arbiter, FIFO write logic, register file). Whether to rewrite existing widgets or leave them alone is a separate decision. Recommendation: rewrite three pilot widgets as a Phase 1 deliverable; leave the rest until rule kernels are validated in production.

**LLM-friendliness.** Rule kernels are arguably *more* LLM-friendly than hand-written FSM kernels because the declarative rule form maps closely to how an LLM would describe behavior in English ("when the FIFO is full, drop the input" → `#[rule] fn drop_when_full(ctx) { guard!(ctx.full); set!(ctx.input_consumed, false); }`). Worth validating empirically once Phase 1 ships.

---

## 19 — Crate organization

A new sibling crate `rhdl-rule` joins the workspace, holding:

- The `#[derive(RuleKernel)]` and `#[rule]` proc-macros.
- The `Reg<T>`, `RuleCtx<W>`, and runtime helper types.
- The `prelude` module re-exporting the user-facing surface.

Like `rhdl-macro` and `rhdl-macro-core`, the crate is split:

- `rhdl-rule` (proc-macro = true): thin entry points.
- `rhdl-rule-core` (regular library): the actual macro implementation, depends on `rhdl-vlog`, `rhdl-span` (per the architecture-document layering rules).

`rhdl-rule-core` does NOT depend on `rhdl-core` (per `architecture.md` §2 — proc-macro support crates cannot depend on the runtime crate).

The user opts into rules per-widget: `use rhdl_rule::prelude::*;` adds the macros.

This addition is a structural change per `architecture.md` §6 (adding a crate). It needs a CHANGELOG entry and reviewer sign-off. The crate is small (~2000 LOC for Phase 1) and well-justified by the design plan.

---

## 20 — References

[1] Hoe, J.C. and Arvind. *Synthesis of Operation-Centric Hardware Descriptions.* ICCAD 2000. — The foundational paper on rule-based hardware synthesis. The conflict matrix and priority-arbitrated scheduler originate here.

[2] Arvind, R.S. Nikhil. *Bluespec System Verilog: Efficient, Correct RTL from High-Level Specifications.* MEMOCODE 2004. — The canonical Bluespec System Verilog reference. The atomic-action / scheduler model that this design plan adopts.

[3] Nikhil, R.S. *Bluespec System Verilog: Efficient, Correct RTL from High-Level Specifications.* (Bluespec, Inc. white paper.) — Practical introduction with worked examples.

[4] Dave, N., Pellauer, M., Nikhil, R.S., Arvind. *Compiling Bluespec.* IBM Research Report, 2005. — The compilation pipeline this design plan loosely follows.

[5] Pellauer, M., et al. *A-Ports: An Efficient Abstraction for Cycle-Accurate Performance Models on FPGAs.* FPGA 2008. — Related rule-like abstraction for performance modeling; useful comparison.

[6] Vijayaraghavan, M., and Arvind. *Bounded Dataflow Networks and Latency-Insensitive Circuits.* MEMOCODE 2009. — The relationship between rule-based scheduling and latency-insensitive design (relevant for the `stream-bus-architecture.md` interlock).

[7] Hoe, J.C. *Operation-Centric Hardware Description and Synthesis.* MIT PhD thesis, 2000. — The doctoral work behind [1]. The most thorough treatment of conflict analysis and scheduler synthesis available.

[8] Bluespec Inc. *Bluespec System Verilog Reference Guide.* (Available from Bluespec Inc.) — The language specification for Bluespec, which we adapt the semantics from.

[9] Augustsson, L. *Compiling Haskell to Bluespec.* — The historical paper on compiling a functional language to rules. Relevant to our adaptation of rules into Rust.

[10] Basu, Samit. *RHDL: Rust as a Hardware Description Language.* LATTE '25, March 2025. (`doc/latte25/latte.tex`.) — The kernel-as-pure-fn invariant this design plan exploits.

---

## 21 — Decisions captured

For the record (also reflected in `architecture.md` and `CLAUDE.md` once shipped):

- **Rules are sugar, not a runtime.** Every `RuleKernel` widget compiles to a regular RHDL `Synchronous` widget. There is no rule-runtime, no rule-interpreter, no scheduler at silicon time — the scheduler is a combinational network synthesized at compile time.
- **Atomicity is guaranteed by the lowering.** A rule's writes either all happen or none of them happen, by construction. There is no path to a partial-rule-firing state.
- **Single clock domain per rule kernel.** All registers and signals in a `RuleKernel<Clk>` carry the same domain `Clk`. Cross-domain communication uses `cdc::*` widgets.
- **Priority-first, not maximal-parallel.** Phase 1 ships strict-priority arbitration. Maximal-parallel-firing is Phase 3; not in v1.
- **`set!` not `write!` for the action macro.** Avoids stdlib clash.
- **Read-set is conservatively extracted.** Any reachable read of a register is a read-set entry. Over-approximation is acceptable; under-approximation is not.
- **The output kernel is separate from the rules.** A `#[output]` method computes the widget's output combinationally from the post-firing state; it is not a rule and does not participate in scheduling.
- **The new crate `rhdl-rule` (split with `rhdl-rule-core`) joins the workspace.** Per `architecture.md` §6 this is a structural change; the design plan is the justification. The crate does not depend on `rhdl-core`.
- **Composes with FSM derive, RCStream, kernel-language extensions, and auto-pipelining without modification.** Each interlock is documented in §3.
- **Phase 1 deliverable includes three widget rewrites** (`core::round_robin_arbiter`, `fifo::write_logic`, one protocol PHY) to validate the lowering on real designs.
