# Static Analysis

Once an enum is `#[derive(Fsm)]` and a widget is `#[derive(FsmWidget)]`, the static-analysis pass can build a transition graph and surface a meaningful class of structural bugs before any cycle is simulated.

## What the analysis catches

| Diagnostic | Meaning |
|---|---|
| `UnreachableState { name }` | A variant that no path from the initial state can reach. Usually means you added a variant but forgot to wire any transition into it. |
| `DeadlockCandidate { name }` | A variant with no outgoing transitions and not marked `#[fsm_state(terminal)]`. Either intentional (mark it terminal) or you forgot the transition out. |
| `SelfLoopSaturation { name }` | A variant whose only outgoing edge is back to itself. Same root cause as the deadlock case — flagged with a more specific message. |
| `NonDeterministicTransition { source }` | More than one match arm produces a different next-state from the same source variant under distinguishable guards. (Reserved for once match guards land — see `kernel-language-extensions.md`.) |
| `Unanalyzable { source, reason }` | The transition extractor couldn't determine the FSM structure for one or more arms. Conservative: surfaces alongside other diagnostics so you know coverage is incomplete. |

Each diagnostic carries the widget name and the source variant it applies to.

## Two-stage architecture

The analysis is split for testability:

1. **Extraction** (`fsm::extraction`) walks the kernel's RHIF opcodes and produces a list of `(source_index, target_index)` pairs plus a list of `(source, reason)` pairs for arms it couldn't analyse.
2. **Analysis** (`fsm::analysis`) consumes the extraction output plus the [`FsmDescriptor`] and emits diagnostics.

You can drive the analysis directly when you have the transition list:

```rust
use rhdl::prelude::*;
use rhdl_core::fsm::analysis::Transition;

let diags = rhdl_core::fsm::analyze_fsm::<MyMachine>(
    &[
        Transition { source_index: 0, target_index: 1 },
        Transition { source_index: 1, target_index: 2 },
        Transition { source_index: 2, target_index: 0 },
    ],
    &[],   // no unanalyzable arms
);
assert!(diags.is_empty());
```

For end-to-end use against a kernel's RHIF, combine `extract_canonical_transitions` with `analyze_fsm_structure`:

```rust
use rhdl_core::fsm::{
    analyze_fsm_structure, extract_canonical_transitions,
};

let result = extract_canonical_transitions(
    &kernel.ops,
    kernel.return_slot,
    &desc,
    &literal_lookup,
);
let diags = analyze_fsm_structure(&desc, &result.transitions, &result.unanalyzable);
```

## What the extractor handles

See the dedicated [Transition Extraction](extraction.md) chapter for the full algorithm and the formal definition of the FSM transition graph.

In short: the extractor walks backward from `d.<state_field>` at the kernel's return point, partitioned by source variant, with constraint propagation through `Case` and `Select` opcodes.  It correctly handles **multiple `match q.<state>` expressions per kernel** (a constraint-propagating walk distinguishes the FSM-transition match from output-computation matches), **the canonical kernel-top default + selective override pattern** (when the widget opts in via `#[fsm(allow_implicit)]`), **guarded transitions with implicit-hold else-branches**, **or-pattern arms**, **`Wild` arms**, and **the canonical `if cr.reset.any() { d.<state_field> = INIT }` reset block** (skipped per convention).

`Unanalyzable` is reserved for genuinely malformed IR: kernel return shape unrecognised, or an `Enum` opcode whose discriminant matches no variant in the descriptor.  The deadlock-candidate check skips any source variant that was unanalyzable, so you don't get noisy false-positive deadlock warnings on patterns the extractor genuinely can't decode.

## Strict mode

By default the diagnostics are warnings. Add `strict` to the widget's `#[fsm(...)]` attribute to escalate them to errors:

```rust
#[derive(Synchronous, SynchronousDQ, FsmWidget)]
#[fsm(state_field = "state", state_enum = State, strict)]
pub struct MyMachine { ... }
```

In strict mode the build fails on the first FSM-structural diagnostic the analysis surfaces.

## Limitations

- **Multiple FSMs in one widget** — a single widget with two distinct state enums is not yet supported.  Split into sub-widgets, each with its own state DFF.  No widgets in the current corpus need this.
- **Match guards** aren't handled (they're a kernel-language extension that hasn't shipped yet — see `kernel-language-extensions.md` §2.4).
- **Cross-DFF over-approximation** — when a kernel has multiple state DFFs and the FSM-tagged one is gated by a condition reading a different state field (e.g., `if q.other_state == X { d.field = Y }`), the extractor includes the `Y` edge for every source variant of the FSM-tagged state, even though by construction only some combinations are reachable.  Sound but verbose in the rendered diagram.  Documented in `fsm-architecture.md` §5.4 #5.

These (and the soundness-rigor follow-up in §5.4.2) are tracked in `fsm-architecture.md` §5.

[`FsmDescriptor`]: ../api/fsm.html
