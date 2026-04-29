# FSM Extraction

The FSM extractor takes a `#[derive(FsmWidget)]`-tagged widget and computes its **transition graph** — the set of `(source variant, target variant)` edges the kernel can produce.  This graph is what every downstream tool consumes: the static analysis (unreachable / deadlock / self-loop saturation), the auto-generated state diagrams, and the SVA emission for formal verification.

This chapter explains what the extractor produces, how it computes it, what guarantees it gives, and where the known acceptance gaps are.

## What is the FSM transition graph?

Given a kernel `K` with declared FSM state field `<state_field>` of enum type `E`, the FSM transition graph `G(K) ⊆ Variants(E) × Variants(E)` is defined formally as:

```text
(s, t) ∈ G(K)  ⟺  ∃ input I such that
                      evaluating K under (q.<state_field> = s, other inputs = I, cr.reset = false)
                      produces  d.<state_field> = t
```

In plain terms: an edge `s → t` exists if and only if there is some input under which the kernel, evaluated with `q.<state_field>` set to `s` and reset deasserted, produces `d.<state_field> = t`.

This definition has three load-bearing properties:

1. **It is about the kernel's I/O behaviour, not its syntax.**  Whether the kernel uses `match q.state`, multiple matches, nested `if`s, a `dont_care()` + field-set construction, or any other RHDL-legal shape, the transition graph is determined by the *function* the kernel computes — not the AST that defines it.  The extractor identifies the relevant data flow by walking back from `d.<state_field>`, not by guessing at syntactic position.

2. **It is about reachability in one cycle, not over time.**  `(s, t) ∈ G(K)` iff the kernel can move from `s` to `t` in a single evaluation.  Multi-cycle reachability is derived by composition — that lives in the leaf analyses (unreachable states, deadlock candidates), not in the extractor.

3. **It excludes the reset path.**  The `cr.reset = false` constraint reflects the convention that reset is treated as out-of-band rather than as a per-state edge.  Diagrams already convey "every state goes to the initial state on reset" via the initial-state marker; including reset edges from every variant would just clutter the graph.

## How the extractor computes it

The algorithm is a backward data-flow walk over the kernel's RHIF, with constraint propagation under each source variant.

### Step 1 — Locate the state slot

The kernel returns a Tuple `(o, d)`.  Walk the d-component slot backward through the chain of `Splice` / `Struct` / `Assign` ops.  The most recent value spliced into path `[<state_field>]` of `d` is the **state slot** — the slot whose value becomes `d.<state_field>` at the kernel's return point.  In SSA this is unambiguous.

If the locate step fails (the kernel return isn't a recognised Tuple → D struct chain), surface a single kernel-level `Unanalyzable` diagnostic with reason and stop.  This catches genuinely malformed kernels without producing a wrong graph silently.

### Step 2 — For each source variant, walk under constraint

For each variant `s` in the descriptor, walk backward from the state slot under the constraint `q.<state_field> = s`.  The constraint propagates through each opcode kind:

| Opcode | Behaviour under constraint `q.<state_field> = s` |
|---|---|
| Literal of state type | Singleton: the literal's discriminant resolves to a variant index |
| `Enum` template | Singleton: same as literal (or per-arm `Unanalyzable` if discriminant matches no variant) |
| `Index` reading `q.<state_field>` | Singleton `{s}` — this produces the implicit self-loop from the canonical kernel-top default `d.<state_field> = q.<state_field>` |
| `Assign(rhs)` | Recurse on rhs |
| `Splice` with `path = [<state_field>]` | Recurse on `subst` (an explicit override) |
| `Splice` with non-state path | Recurse on `orig` (the state field is held) |
| `Struct` with explicit `<state_field>` member | Recurse on the member's value |
| `Struct` without explicit `<state_field>` | Empty (field comes from template, typically `dont_care`) |
| `Select` with reset condition | Walk only the false-branch (skip reset-override; see "Reset handling" below) |
| `Select` with `q.<state_field> == X` condition | Statically resolved under the constraint; walk only the matching branch |
| `Select` with opaque condition | Union of both branches (sound over-approximation) |
| `Case` with discriminant reading `q.<state_field>` | Only the arm whose `CaseArgument` matches `s`'s discriminant contributes (or the `Wild` arm) — other arms constraint-eliminated |
| `Case` with non-state discriminant | Union of all arms |
| Any other opcode | Empty (no info from this slot) |

The walker is recursive over RHIF in SSA form, so termination is guaranteed by the op-count bound.

### Why constraint propagation matters

Production widgets routinely have **multiple `match q.<state>` expressions** per kernel — output computation, phase classification, the actual transition logic, and so on.  Quick survey of the corpus:

| widget | `match q.<state>` count | comment |
|---|---|---|
| `can_master` | 5 | raw_bit + in_stuff_zone + crc_input_active + transition + dlc_reg-match |
| `i2c_master` | 4 | in_byte_phase + sample-time + advance-time + sda_drive |
| `half_spi_master` | 5 | transition + cs_n + sclk + sdio_oe + busy |
| `hd44780` | 5 | similar PHY shape |

A heuristic that "finds the first `Case` opcode and reads its arms as transitions" works on toy widgets but breaks immediately on real production kernels: it picks up the *output-computation* match (`raw_bit`, returning `bool`) instead of the *transition* match (returning a state value).  Constraint propagation distinguishes them automatically:

- For an output-computation match like `let raw_bit = match q.field { Sof => false, Id => bit, ... }`: under the source-variant constraint, the `Case` reduces to a single bool value.  That bool isn't state-typed, so the walker yields empty for state values.  The output match contributes nothing to the transition graph.
- For the transition match: under the constraint, the `Case` reduces to the matching arm's d-struct.  Walking that yields the arm's transition target(s).

The extractor doesn't need to know syntactically which match is "the" transition match — the constraint propagation does the right thing for any shape automatically.

### Reset handling

The canonical RHDL reset block:

```rust
if cr.reset.any() {
    d.<state_field> = INIT;
    /* other field resets */
}
```

lowers to RHIF as:

```text
r70 <- r0.reset                  # Index(cr, [.reset])
r69 <- |r70                      # Unary(OrReduce, r70)  i.e. .any()
r71 <- Splice(d, [<state_field>], INIT_enum)
r76 <- Select(r69, r71, d_normal)
```

The walker recognises this shape via `slot_reads_reset_field`: when a `Select`'s condition slot traces back (through `Unary`, `Index`, `Assign`) to an `Index` reading `.reset` of any struct, the walker treats that Select as a reset-override and walks **only the false-branch**.  The reset-override d-struct is excluded.

This honours the convention: the FSM transition graph is the non-reset behaviour graph.  Reset is an architectural constant, not a per-state edge.

### Implicit self-loops — opt-in via `#[fsm(allow_implicit)]`

The canonical RHDL kernel pattern (CLAUDE.md §3) is to construct `d` via `dont_care()`, write the kernel-top default `d.<state_field> = q.<state_field>` once, and only override it in arms that transition.  An arm that omits the `d.<state_field>` write — or a conditional inside an arm whose else-branch quietly skips the assignment — produces an *implicit self-loop*: the data-flow walk falls through to the kernel-top default, which under the constraint `q.<state_field> = s` evaluates to `s`.

Whether implicit self-loops are included in the extracted graph is a per-widget choice, controlled by `#[fsm(allow_implicit)]`:

```rust
#[derive(Clone, Debug, Synchronous, SynchronousDQ, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = State, allow_implicit)]   // ← opt in
pub struct MyWidget {
    state: dff::DFF<State>,
    /* other fields */
}
```

**With `allow_implicit`:** the canonical pattern is recognised; arms that hold the state via the kernel-top default produce `(s, s)` edges in the graph.  Use this when the widget actually wants stay-in-place behaviour from the kernel-top default — typical for protocol PHYs (CAN, I²C, SPI, UART, etc.) where most clock cycles are "wait for the next event."

**Without `allow_implicit` (default):** the extractor only emits transitions for *explicit* writes to `d.<state_field>` inside an arm.  Arms that fall through to the kernel-top default contribute nothing.  A state whose only would-be outgoing edge was an implicit self-loop appears with no outgoing edges; the analysis layer fires `DeadlockCandidate` for it.  This catches forgotten transitions — the case where the author meant to write `d.state = State::Next` but didn't, and the kernel silently holds in place.

The default is **strict** (no implicit self-loops).  Widgets that rely on the canonical pattern declare it explicitly; the attribute documents intent.

**Worked example** — `can_master::Id` arm:

```rust
CanField::Id => {
    if q.field_bit_idx == bits::<7>(10) {
        d.field = CanField::Rtr;       // explicit transition
        d.field_bit_idx = zero_b7;
    } else {
        d.field_bit_idx = next_idx;    // no d.field write — implicit hold
    }
}
```

With `#[fsm(allow_implicit)]` on `CanMaster`: the walker sees the `Select` whose true-branch produces `Splice([field], Rtr)` and whose false-branch holds the field via the kernel-top default.  Under the constraint `q.field = Id`, both branches union to `{Rtr, Id}` — yielding edges `Id → Rtr` and `Id → Id`.

Without `#[fsm(allow_implicit)]`: only the explicit `Id → Rtr` edge is emitted.  If the kernel's *only* path out of `Id` were the implicit hold (no explicit transition anywhere), `Id` would appear with zero outgoing edges and the analysis layer would fire `DeadlockCandidate` — exactly what you'd want if the author had forgotten to wire the transition.

**Composition with `strict`.**  `#[fsm(strict, allow_implicit)]` gives strict-mode error escalation on a graph that includes implicit holds.  `#[fsm(strict)]` alone (without `allow_implicit`) escalates the deadlock-on-no-explicit-transitions case to a hard error at synthesis time — the most paranoid configuration.

## Cross-DFF over-approximation

The principled algorithm is sound (no false negatives) but may produce **extra edges that won't fire under reasonable inputs** when the kernel has cross-DFF state interactions.  The motivating example is `can_master`'s outer guard:

```rust
if q.state == CanState::Idle {
    if i.start {
        d.state = CanState::Tx;
        d.field = CanField::Sof;       // ← this writes d.field
        ...
    }
}
```

The kernel has two state DFFs: `state: CanState` (Idle/Tx) and `field: CanField` (the FSM the descriptor declares).  The outer condition reads `q.state` (the *other* state field), which the extractor's constraint propagation can't statically resolve under the FSM's source-variant constraint (`q.field = s`).  So both branches of the outer Select are unioned, and every `CanField` source variant ends up with an edge back to `CanField::Sof`.

By construction, `q.state == CanState::Idle` only ever co-occurs with `q.field == CanField::Sof` (the broader system invariant: when the CAN master is idle, the field counter is at the start of frame).  The "every-state-back-to-Sof" edges are therefore **unreachable in practice** but **possible per the I/O definition**.  The extractor includes them; the rendered diagram shows them.

This is documented as the over-approximation budget in `fsm-architecture.md` §5.4 #5.  A future Layer-3 enhancement could deemphasise these edges in the rendered diagram by recognising the syntactic shape (edges where the source path traces through `if q.<other_state_field> == X`).

## When `Unanalyzable` fires

After this PR, `Unanalyzable` is reserved exclusively for cases where the static analysis cannot derive a sound answer:

- **Kernel-level**: the return shape isn't a recognised Tuple → D struct chain, OR the d-component chain never overrides the state field.  Surfaces a single `<kernel>`-named diagnostic.
- **Per-arm**: an `Enum` opcode whose discriminant value matches no variant in the descriptor (the kernel emits a state value the type system says shouldn't exist).

Genuine "no transition info" (an arm that simply holds the state in place via the canonical kernel-top default) is *not* `Unanalyzable` — it's an implicit self-loop, which the principled algorithm computes correctly.

## Validation tiers

Three tiers of testing pin the extractor's correctness:

### Tier 1 — Synthetic-RHIF unit tests

`crates/rhdl-core/src/fsm/extraction.rs` `tests` mod.  Each test constructs a synthetic RHIF op stream by hand and asserts the extractor's output.  Covers:

- The motivating multi-match dispatch (output-computation `Case` ignored)
- Constraint propagation through `Case` and `Select` (including state-eq comparisons in both operand orders)
- Kernel-top-default-only kernels (all self-loops)
- Guarded transitions with implicit-hold else-branches
- Or-pattern arms, `Wild` arms
- Reset block detection (Unary + Index chain for `cr.reset.any()`)
- EnumDiscriminant chain in `Case` discriminants (the `#` extraction op)
- Locate-step traversal through non-state Splices
- Negative tests: non-Tuple return, unmatched enum discriminant, locate-step failure
- The implicit-hold-masks-deadlock acceptance gap (pinned for the follow-up that lands explicit-vs-implicit self-loop tracking)

### Tier 2 — Adversarial integration widgets

`crates/rhdl-fpga/src/doc.rs` `tests` mod.  ~10 small `#[derive(FsmWidget)]` widgets covering distinct kernel-language idioms (side-effect form, let-binding form, nested if-else, mixed arms, can_master-shape, etc.).  Each widget is compiled through Stage 1, the extractor runs, and the output is asserted against the expected transition list.

### Tier 3 — Property-based tests against the simulator

`crates/rhdl-fpga/src/doc.rs` includes property-based tests that enumerate every `(source variant, input)` combination for representative widgets, call the kernel function directly via the `DigitalFn3::func()` interface, observe `d.<state_field>` after the call, and assert that every simulator-observed transition is in the extractor's output.

This validates **soundness against the executable semantics**: every transition the simulator can actually produce IS in the extractor's graph.  Catches false negatives that pure structural tests can't.  Per `fsm-architecture.md` §5.4.2.

### Tier 4 — Corpus snapshot regression (downstream PR)

The widget reorganization PR ships `crates/rhdl-fpga/src/fsm_corpus_regression.rs` with one snapshot test per FSM-tagged widget in the production corpus.  Currently 27 widgets (audio, core, serial_bus families).  Each test asserts the derived graph matches the blessed snapshot AND zero `Unanalyzable` diagnostics.  Refresh via `UPDATE_EXPECT=1`.

## API

The two main entry points are:

```rust
pub fn extract_widget_transitions<W>() -> Result<ExtractionResult, RHDLError>
where
    W: FsmWidget + SynchronousIO,
```

Compiles `W::Kernel` through Stage 1 and runs the extractor.  Returns `ExtractionResult { transitions, unanalyzable }`.

```rust
pub fn extract_widget_transitions_strict<W>() -> Result<Vec<Transition>, RHDLError>
where
    W: FsmWidget + SynchronousIO,
```

Same, but errors out on any `Unanalyzable` diagnostic.  Use this when you want a single-call "give me the transitions or fail loudly" semantic — typically inside diagram-emission helpers and drift-check tests.

The downstream consumers — `write_fsm_diagram::<W>(filename)` for diagram emission and `analyze_fsm_structure(...)` for structural diagnostics — both call into these.

## Known acceptance gaps (necessary follow-ups)

The deadlock-masking gap (previously documented in `fsm-architecture.md` §5.4.1) is now **closed** by the `#[fsm(allow_implicit)]` opt-in described above: with the default `allow_implicit = false`, implicit self-loops disappear from the graph and `DeadlockCandidate` fires correctly on states with no explicit outgoing transitions.  Widgets that genuinely want the canonical implicit-hold pattern opt in explicitly.

One structural gap remains, tracked in `fsm-architecture.md` §5.4.2 as NECESSARY follow-up work.

### Soundness for arbitrary kernels (vs the canonical patterns)

The extractor is validated against the executable semantics for the canonical kernel patterns and against the RHIF data-flow shape it expects.  Soundness for *arbitrary* kernels (and stability against future RHIF lowering changes) requires:

- **Reset detection beyond the canonical pattern.**  The current detection is a structural pattern match for `Select(Unary(OrReduce, Index(_, [.reset])), ...)`.  A kernel using a non-canonical reset shape would either be missed (producing extra edges) or false-positive.  Either constrain by enforcement (a Layer 2 diagnostic that flags non-canonical reset shapes) or generalise the detection (semantic rather than structural recognition of "reset condition").
- **Property-based testing across more widget shapes.**  This PR ships property-based tests for two representative widgets; extending coverage to every adversarial widget in `doc.rs` (and to the corpus once it lands on main) would tighten the empirical soundness validation further.
- **Formal RHIF semantics + a proof of the algorithm's soundness.**  RHDL doesn't have a formal RHIF semantics yet.  Without it, every static analysis on RHIF is "structurally plausible against the patterns we tested" rather than "proven sound for all kernels."  Research-grade work; flagged as the asymptote, not committed.

## Prior implementations and their limitations

Three iterations preceded the current principled algorithm:

- **PR #2 (`feat/fsm-architecture`)** shipped the v1 extractor recognising only the *let-binding form* (`let next = match q.state { ... }; d.state = next;`) by finding the first `Case` opcode and reading its arms.  Excluded ~95% of real RHDL widgets, which use the side-effect form.
- **PR #6 (`feat/fsm-extractor-side-effects`)** extended the heuristic with a side-effect-form walker (Splice path-targeting `[<state_field>]` etc.).  Validated only against synthetic widgets; first live test against `core::can_master` produced spurious `Unanalyzable` on 4 of 13 arms.
- **PR #7 (`feat/fsm-extractor-implicit-self-loops`)** added implicit-self-loop semantics to fix the `can_master` `Unanalyzable` regression, but the heuristic was still "find the first Case opcode" — which on real `can_master` selects the `raw_bit` output-computation match, producing 13 wrong transitions out of 20.

The principled algorithm replaces all of this with the data-flow walk in this chapter, validated against the full 27-widget corpus.

[`FsmDescriptor`]: ../api/fsm.html
