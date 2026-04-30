# `Exec(lhs, id, args)`

Call a kernel function. The callee is identified by `id`, an opaque index into the surrounding `Object`'s `externals` table.

## Syntax

```
Exec { lhs: Slot, id: FuncId, args: Vec<Slot> }
```

## Type rule

```
externals[id] = func    -- func is itself a well-formed Object
|args| = |func.arguments|
∀ i. Γ ⊢ args[i] : kind_of(func.arguments[i])
Γ ⊢ lhs : kind_of(func.return_slot)
———————————————————————————————————————————————————————
Γ ⊢ Exec(lhs, id, args) : ok
```

The argument count and per-argument kinds must match the callee's declared signature; the result kind matches the callee's return slot.

## Dynamic semantics

```
∀ i. read(σ, args[i]) = vᵢ
result = execute(externals[id], [v₀, …, vₙ])
————————————————————————————————————————————————————
σ ⊢ Exec(lhs, id, args) ⇓ σ[lhs ↦ result]
```

`execute` (defined in `vm.rs`) recursively runs the callee in a fresh state with the supplied argument values, then returns `read(σ_callee, callee.return_slot)`.

A kernel call is a black box from the caller's perspective: the callee's local state, its register slots, and its intermediate computations are inaccessible. Only the result is observed.

## Pre-conditions

- `externals[id]` is defined and is itself a well-formed `Object`.
- Argument count and kinds match the callee.
- The callee terminates (always true for well-formed RHIF — recursion is forbidden, loops are bounded, and the call graph is acyclic; see [`invariants/object.md`](../invariants/object.md)).

## Post-conditions

- `lhs` is bound to the callee's return value.
- The caller's state is otherwise unchanged.

## Notes

- **No recursion.** The front-end forbids a `#[kernel]` from calling itself, directly or indirectly. The `externals` map is a finite acyclic graph of kernels.
- **No mutation.** Kernels are pure; calls have no observable effect beyond producing the return value.
- **No partial application, no closures.** Every `Exec` provides all arguments by value at compile time. There is no slot of "kernel" kind.

## Lowering

`Exec` lowers to either:

- **Inlining**, when the callee is small enough or marked `#[inline]`-equivalent. The callee's body is spliced into the caller's RHIF, with arguments rewired and the return slot routed.
- **Module instantiation**, when the callee should remain a separate module in the synthesised RTL. Each `Exec` becomes a Verilog module instance with the argument slots wired to the input ports and the result slot wired to the output port.

The choice between inlining and instantiating is governed by passes in `rhdl-core::compiler`.

## Cross-references

- `Exec` and `FuncId` in `spec.rs`.
- `vm.rs::execute` for the recursive call.
- `Object::externals` for the callee table.
