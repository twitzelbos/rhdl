# `#[derive(Fsm)]` and `#[derive(FsmWidget)]`

The FSM tooling is opt-in via two derive macros. Both are purely additive: they emit metadata trait impls without changing the kernel body or the widget's `Synchronous` impl.

## State enum: `#[derive(Fsm)]`

Apply it alongside `#[derive(Digital)]` on the enum that names the FSM's states.

```rust
use rhdl::prelude::*;

#[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
pub enum State {
    #[default]
    Idle,
    Running { counter: b8 },
    Done,
}
```

This emits an `impl FsmState for State` carrying:

- a static `&'static [FsmVariantDescriptor]` listing each variant in source order,
- the index of the initial variant (the one marked `#[default]`, or 0 if no `#[default]` is present),
- a `fsm_variant_index(&self)` method that maps a value back to its variant index.

### Per-variant decoration

Each variant can carry an optional `#[fsm_state(...)]` attribute with two recognised arguments:

- `label = "..."` — a human-readable label that the diagram renderer uses instead of the variant name.
- `terminal` — marks the variant as intentionally absorbing. The static-analysis pass skips it for the deadlock-candidate check.

```rust
#[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
pub enum State {
    #[default]
    #[fsm_state(label = "idle, waiting for start")]
    Idle,
    #[fsm_state(label = "running, counter = {counter}")]
    Running { counter: b8 },
    #[fsm_state(label = "complete", terminal)]
    Done,
}
```

### Overriding the initial variant

If the FSM's initial state isn't the `#[default]` variant, override with `#[fsm(initial = "VariantName")]`:

```rust
#[derive(Fsm, Digital, PartialEq, Copy, Clone, Debug, Default)]
#[fsm(initial = "Running")]
pub enum State {
    #[default]   // serialisation default — but the FSM starts elsewhere
    Idle,
    Running,
    Done,
}
```

The macro verifies the named variant exists; if you typo the name, the macro fails to compile with a precise error.

## Widget struct: `#[derive(FsmWidget)]`

Apply it on the widget struct that owns the state DFF, alongside `#[derive(Synchronous, SynchronousDQ)]`. Two attribute arguments are required:

- `state_field = "..."` — the name of the field holding the state DFF.
- `state_enum = ...` — the enum type that the state DFF carries. Accepts either a path (`State`, `crate::states::State`) or a string literal (`"crate::states::State"`).

```rust
use rhdl::prelude::*;

#[derive(Synchronous, SynchronousDQ, FsmWidget)]
#[fsm(state_field = "state", state_enum = State)]
pub struct MyMachine {
    state: dff::DFF<State>,
    // ...
}
```

This emits an `impl FsmWidget for MyMachine` with a `fsm_descriptor()` associated function returning the widget's compiled [`FsmDescriptor`]. The descriptor is what the static-analysis pass and the diagram generator consume.

### Strict mode

Add `strict` to the attribute to escalate the static-analysis diagnostics from warnings to errors:

```rust
#[derive(Synchronous, SynchronousDQ, FsmWidget)]
#[fsm(state_field = "state", state_enum = State, strict)]
pub struct MyMachine { ... }
```

In strict mode any unreachable state or deadlock candidate becomes a build-breaking error rather than an advisory warning.

## What you don't have to write

Both derives are *metadata only*. They don't:

- generate kernel code,
- modify the widget's `Synchronous` impl,
- introduce any runtime cost beyond a `&'static` slice per FSM,
- require any new `#[kernel]`-internal syntax.

The kernel that walks `match q.state` looks exactly the same whether the FSM tooling is opted in or not. That's deliberate — see `fsm-architecture.md` §4.1 for the rationale.

[`FsmDescriptor`]: ../api/fsm.html
