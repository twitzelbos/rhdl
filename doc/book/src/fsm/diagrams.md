# State Diagrams

For every widget tagged with `#[derive(FsmWidget)]`, the FSM tooling can render a state-transition diagram in three formats:

- **Inline SVG** — embedded directly in the widget's rustdoc, so reading the docs for a widget shows you its FSM at a glance. No external Graphviz dependency at build time.
- **Graphviz `dot`** — for users who want to pipe through `dot`, `xdot`, or any other Graphviz consumer.
- **Structured JSON** — for LLM-driven workflows that want to consume the FSM structure programmatically.

## Building the diagram

The diagram is computed from an [`FsmDescriptor`] (which the `#[derive(FsmWidget)]` macro emits) plus the transition list (which the static-analysis extractor produces). The `build_fsm_diagram` helper combines them into a laid-out [`FsmDiagram`]:

```rust
use rhdl::prelude::*;
use rhdl_core::fsm::analysis::Transition;
use rhdl_core::fsm::diagram::{
    build_fsm_diagram, render_fsm_dot, render_fsm_json, render_fsm_svg,
};

let desc = MyMachine::fsm_descriptor();
let transitions = [
    Transition { source_index: 0, target_index: 1 },
    Transition { source_index: 1, target_index: 2 },
    Transition { source_index: 2, target_index: 0 },
];
let diagram = build_fsm_diagram(&desc, &transitions);

let svg = render_fsm_svg(&diagram);
let dot = render_fsm_dot(&diagram);
let json = render_fsm_json(&diagram);
```

## Layout

The renderer uses a layered breadth-first layout:

- The **initial variant** sits at the top of the diagram.
- Each variant's layer is its BFS depth from the initial variant.
- Variants unreachable from the initial state get tucked into a final "orphan" layer so they're still visible.
- **Self-loops** are rendered as small round-tripping arcs above the node rather than overlapping straight lines.
- The **initial variant** has a blue border and tinted fill; **terminal** variants (marked `#[fsm_state(terminal)]`) get a green fill and bold border.

The layout is deliberately simple — "good enough for at-a-glance rustdoc inspection". For production-quality rendering, dump the `dot` form and feed Graphviz manually:

```sh
cargo run --example my_widget_dump_dot | dot -Tsvg > diagram.svg
```

## JSON for LLM workflows

The structured JSON form is intended for programmatic consumption — for example, by an LLM agent that wants to read the FSM's structure alongside the source when proposing changes. The schema is hand-rolled (no `serde_json` dependency in `rhdl-core`), small, and deterministic:

```json
{
  "widget": "MyMachine",
  "initial_index": 0,
  "nodes": [
    {"index": 0, "name": "Idle", "label": "Idle", "terminal": false, "has_payload": false},
    {"index": 1, "name": "Running", "label": "Running", "terminal": false, "has_payload": true},
    {"index": 2, "name": "Done", "label": "complete", "terminal": true, "has_payload": false}
  ],
  "edges": [
    {"source": 0, "target": 1, "self_loop": false},
    {"source": 1, "target": 2, "self_loop": false},
    {"source": 2, "target": 0, "self_loop": false}
  ]
}
```

[`FsmDescriptor`]: ../api/fsm.html
[`FsmDiagram`]: ../api/fsm.html
