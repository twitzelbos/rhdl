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

let result = extract_canonical_transitions(&kernel.ops, &desc, &literal_lookup);
let diags = analyze_fsm_structure(&desc, &result.transitions, &result.unanalyzable);
```

## What the canonical extractor handles

The current extractor recognises:

- A single `match` on the state field (`match q.state { ... }`).
- Each arm whose result is constructed by a single `Enum` opcode (i.e., the next state is a plain variant constructor like `State::Running { counter: 0 }`).
- `Assign`-forwarded arm results (intermediate variable bindings).
- Wild arms (`_`) — silently skipped because they don't contribute a definite transition target.

Anything else — nested if/else producing the next state, field-by-field assignment to a `dont_care()`-built state struct, computed transitions via `match` inside `match` — is conservatively reported as `Unanalyzable`. The deadlock-candidate check skips any source variant that was unanalyzable, so you don't get noisy false-positive deadlock warnings on patterns the extractor can't yet decode.

## Strict mode

By default the diagnostics are warnings. Add `strict` to the widget's `#[fsm(...)]` attribute to escalate them to errors:

```rust
#[derive(Synchronous, SynchronousDQ, FsmWidget)]
#[fsm(state_field = "state", state_enum = State, strict)]
pub struct MyMachine { ... }
```

In strict mode the build fails on the first FSM-structural diagnostic the analysis surfaces.

## Limitations (v1)

- The extractor recognises one `match` per kernel. Kernels with multiple state-machines composed in the same function are not yet supported (split them into sub-widgets, each with its own state DFF).
- Only the immediate-defining opcode of each arm result is inspected; nested case analysis is conservatively skipped.
- Match guards aren't handled (they're a kernel-language extension that hasn't shipped yet — see `kernel-language-extensions.md` §2.4).

These are tracked as v2 follow-ups in `fsm-architecture.md` §10.

[`FsmDescriptor`]: ../api/fsm.html
