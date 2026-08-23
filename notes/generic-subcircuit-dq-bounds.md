# A widget cannot be generic over a sub-circuit

Found while trying to let `dsp::ddc::Ddc` accept either
`cic::CicDecimate` or a `cic_pruned!`-generated decimator. It cannot,
and the reason is a two-line defect in the DQ derives rather than
anything structural.

## What was attempted

```rust
#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
pub struct Outer<C>
where C: SynchronousIO<I = bool, O = bool> + Synchronous + Default + Clone + Debug,
{
    inner: C,
    reg: dff::DFF<bool>,
}
```

The sub-circuit's I/O types are *pinned* by the bound, so nothing about
the kernel is generic — `q.inner` is a `bool` whatever `C` is.

## Why it fails

`derive_synchronous_dq` already does the right thing. It projects:

```rust
#[derive(Digital, Clone, Copy, PartialEq)]
pub struct Q<C> { inner: <C as SynchronousIO>::O, reg: bool }
```

The field type does not mention `C` once the projection is normalised.
But `#[derive(Copy)]` and `#[derive(PartialEq)]` add bounds on the
*type parameter*, not on the *field types* — the standard derive
behaviour that the "perfect derive" proposal exists to fix. So the
generated code demands `C: Copy` and `C: PartialEq`, and `C` is a
circuit, which is never either.

With every satisfiable bound added by hand, exactly these remain:

```
error[E0277]: can't compare `C` with `C`
error[E0277]: the trait bound `C: Copy` is not satisfied
error[E0277]: the trait bound `D<C>: Copy` is not satisfied
error[E0369]: binary operation `==` cannot be applied to type
              `&mut (Q<C>, <C as Synchronous>::S, dff::S<bool>)`
```

All four are the same cause. The last one is `Synchronous`'s derived
`S` tuple inheriting the same problem.

## The fix

Emit the `Clone`/`Copy`/`PartialEq`/`Digital` impls for `Q`/`D`/`S`
with where-clauses over the *field types* rather than relying on
`#[derive]`'s parameter bounds — either by hand-writing the impls in
`synchronous_dq.rs` and `circuit_dq.rs`, or by adding
`where <C as SynchronousIO>::O: Copy, ...` to the generated structs and
suppressing the default bounds.

This is `rhdl-macro-core`, so per CLAUDE.md §11.1 it is its own PR:
tests at the macro-snapshot and kernel-integration levels, a
Justification section, and an audit of every widget whose `Q`/`D`
generated code shifts. The blast radius is wide — every `SynchronousDQ`
widget in the tree regenerates — even though no *behaviour* should
change, which makes the widget Tier-3 snapshots the thing to watch.

## What it unblocks

Composition over sub-circuits generally, and specifically letting one
`Ddc` host either the uniform or the pruned decimator instead of
needing a second copy of the DDC kernel. Until then a pruned DDC would
mean duplicating ~100 lines of kernel that must stay in sync, which is
worse than waiting.
