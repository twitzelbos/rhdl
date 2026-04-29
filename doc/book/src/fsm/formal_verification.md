# Formal Verification

The FSM tooling exposes a SystemVerilog Assertion (SVA) surface so you can prove temporal properties about your machine — invariants, liveness goals, coverage points, and environment assumptions — and feed them into a SymbiYosys-driven proof flow.

This is Layer 4 of the design plan in `fsm-architecture.md`. The metadata layer is shipped; the cargo subcommand that drives SymbiYosys end-to-end is a follow-up.

## Declaring properties

The `#[fsm_properties(...)]` attribute on a kernel function records up to four kinds of properties:

```rust
use rhdl::prelude::*;

#[fsm_properties(
    invariant("state != State::Error", name = "no_error"),
    cover("state == State::Done"),
    liveness("state == State::Done", bound = 1024),
    assume("input.valid"),
)]
#[kernel]
pub fn my_machine(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    /* ... */
}
```

| Kind | SVA primitive | Meaning |
|---|---|---|
| `invariant("...")` | `assert property` | Boolean expression that must hold every cycle. |
| `liveness("...", bound = N)` | `assert property ##[1:N]` | Property that must hold within `N` cycles. Omit `bound` for unbounded `s_eventually`. |
| `cover("...")` | `cover property` | Coverage point — does the design ever satisfy this? Useful as both verification and dead-code detection. |
| `assume("...")` | `assume property` | Environment assumption the proof can rely on. |

Each declaration takes a string-literal expression as its primary argument, plus optional named arguments:

- `name = "..."` — gives the property a stable label in the SVA output and in diagnostics. If omitted, an auto-generated label like `<kernel>_prop_<index>` is used.
- `bound = N` — for liveness properties, the cycle window. Ignored for other kinds.

## The expression sublanguage

The expression body of each property is a strict subset of the kernel-accepted expression language:

- equality (`==`, `!=`),
- comparison (`<`, `<=`, `>`, `>=`),
- Boolean operators (`&&`, `||`, `!`),
- field access (`output.busy`, `state.counter`),
- `matches!`,
- variant patterns (`State::Running { .. }`),
- *no calls* — to keep symbolic execution tractable in Layer 5.

For v1 the expression is passed through verbatim into the SVA emission; the SystemVerilog parser at the SymbiYosys layer rejects anything unsupported. v2 will add a small AST + grammar check at compile time so the user gets RHDL-style error messages instead of Yosys-style ones.

## What gets emitted

Each kernel that carries `#[fsm_properties(...)]` exposes its property table via the [`FsmKernelProperties`] trait. The marker type follows the convention `FsmProps_<kernel_name>`:

```rust
let props = <FsmProps_my_machine as FsmKernelProperties>::fsm_properties();
let sva  = rhdl_core::fsm::render_property_sva(props);
println!("{sva}");
```

The renderer produces SVA wrapped in `// pragma rhdl-fsm-property begin` / `// pragma rhdl-fsm-property end` markers so downstream tooling can splice the block in or out:

```systemverilog
// pragma rhdl-fsm-property begin
assert property (no_error_p) (@(posedge clk) state != State::Error);
cover  property (my_machine_prop_1_p) (@(posedge clk) state == State::Done);
assert property (my_machine_prop_2_p) (@(posedge clk) ##[1:1024] state == State::Done);
assume property (my_machine_prop_3_p) (@(posedge clk) input.valid);
// pragma rhdl-fsm-property end
```

## Wiring into SymbiYosys

The emitted SVA is suitable for splicing into a generated Verilog module's body, then handing the result to SymbiYosys:

```sh
sby -f my_design.sby
```

A `cargo rhdl prove` subcommand that automates this — generates Verilog with SVA included, writes a `.sby` config, invokes `sby`, and structures the counterexample trace — is a follow-up in the FSM track. Until it lands, the property metadata is available for any user-built tooling via the `FsmKernelProperties` trait and `render_property_sva` helper.

## Why SymbiYosys?

It's the canonical open-source formal-verification frontend for Verilog/SystemVerilog. Driven by Yosys; supports multiple proof engines (smtbmc, abc-pdr, z3, boolector, yices, cvc4); handles bounded and unbounded model checking and k-induction; and is the verification toolchain that nMigen/Amaranth, Spade, and SymbiFlow all use. Free, mature, well-documented. Building our own model checker is Layer 5 of the design plan; integrating with SymbiYosys is Layer 4.

[`FsmKernelProperties`]: ../api/fsm.html
