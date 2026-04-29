# Finite State Machines

A large fraction of every hardware widget is a finite-state machine in disguise — protocol PHYs, arbiters, FIFO read/write logic, register-mapped peripherals all step through a small set of named states under input-driven transitions. RHDL's FSM tooling makes that pattern first-class:

- The `#[derive(Fsm)]` macro tags a `Digital`-derived enum as a state type and records its variant table for downstream tooling.
- The `#[derive(FsmWidget)]` macro on a `Synchronous`-derived widget struct tells the tooling which field holds the state DFF and which enum it carries.
- The static-analysis pass walks the kernel's match-on-state, builds the transition graph, and reports unreachable states, deadlock candidates, and self-loop saturation.
- The diagram generator turns that graph into inline SVG (for rustdoc), Graphviz `dot` (for piping into your own graph tooling), and a structured JSON form (for LLM-driven workflows).
- The `#[fsm_properties(...)]` attribute lets you declare invariants, liveness goals, coverage points, and environment assumptions next to the kernel; the property-rendering helper produces the corresponding SystemVerilog Assertions for SymbiYosys-driven formal verification.

The chapters in this section cover each piece in turn. The contract is *additive metadata*: the kernel body itself is unchanged, the macros declare structure, and the analyses + renderers consume that structure to produce diagnostics, diagrams, and proof obligations.

For the architectural design behind these features, see `fsm-architecture.md` at the workspace root.
