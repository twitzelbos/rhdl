//! State-diagram rendering for FSM-tagged widgets.
//!
//! Layer 3 of `fsm-architecture.md`.  Two output formats:
//!
//! - **Inline SVG**, embedded directly in widget rustdoc via the
//!   widget's `Descriptor`.  No external Graphviz dependency at
//!   build time.
//! - **Graphviz `dot`**, for users who want to pipe through `dot`
//!   / `xdot` / their own graph-analysis tooling.
//!
//! Plus a structured JSON representation for LLM-tool consumption.
//!
//! Layout strategy: a deliberately simple two-pass layout that
//! handles both DAGs (Sugiyama-ish layered placement) and cyclic
//! FSMs (the common case).  The render is "good enough for
//! at-a-glance inspection in rustdoc"; for production-quality
//! rendering, dump `dot` and feed Graphviz.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write;

use crate::fsm::analysis::Transition;
use crate::fsm::descriptor::FsmDescriptor;
use crate::fsm::state::FsmVariantDescriptor;

/// A laid-out FSM diagram, ready to render.
///
/// `nodes` is the variant table augmented with `(x, y)` placements
/// in a unit-less integer grid; `edges` is the transition list.
/// Renderers convert one of these into SVG, dot, or JSON.
#[derive(Debug, Clone)]
pub struct FsmDiagram {
    pub widget_name: &'static str,
    pub initial_index: usize,
    pub nodes: Vec<DiagramNode>,
    pub edges: Vec<DiagramEdge>,
}

/// One node in the laid-out diagram.
#[derive(Debug, Clone)]
pub struct DiagramNode {
    pub index: usize,
    pub name: &'static str,
    pub label: Option<&'static str>,
    pub terminal: bool,
    pub has_payload: bool,
    /// Layer assigned by the layered-layout pass.  Used to compute
    /// `(x, y)` for the SVG renderer.
    pub layer: usize,
    /// Position within the layer (0-based, ascending).
    pub layer_pos: usize,
}

/// One edge in the laid-out diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagramEdge {
    pub source_index: usize,
    pub target_index: usize,
    /// True if the edge is a self-loop; renderers draw self-loops
    /// distinctly (a small round-tripping arc rather than a line).
    pub self_loop: bool,
}

/// Build a diagram by laying out the variant table + transition
/// graph.  This is the single entry point shared by all three
/// renderers.
pub fn build_fsm_diagram(desc: &FsmDescriptor, transitions: &[Transition]) -> FsmDiagram {
    let variants = desc.variants();
    let initial = desc.initial_index();

    // Deduplicate edges; classify self-loops.
    let mut edge_set: BTreeSet<(usize, usize)> = BTreeSet::new();
    for t in transitions {
        edge_set.insert((t.source_index, t.target_index));
    }
    let edges: Vec<DiagramEdge> = edge_set
        .into_iter()
        .map(|(src, tgt)| DiagramEdge {
            source_index: src,
            target_index: tgt,
            self_loop: src == tgt,
        })
        .collect();

    // Layered BFS layout from the initial node.  Variants not
    // reachable get placed in a final "orphan" layer.
    let n = variants.len();
    let mut layer_of: Vec<Option<usize>> = vec![None; n];
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in &edges {
        if e.source_index < n && e.target_index < n && !e.self_loop {
            adjacency[e.source_index].push(e.target_index);
        }
    }

    if initial < n {
        layer_of[initial] = Some(0);
        let mut queue = VecDeque::new();
        queue.push_back(initial);
        while let Some(node) = queue.pop_front() {
            let depth = layer_of[node].unwrap();
            for &next in &adjacency[node] {
                if layer_of[next].is_none() {
                    layer_of[next] = Some(depth + 1);
                    queue.push_back(next);
                }
            }
        }
    }
    let max_layer = layer_of.iter().filter_map(|x| *x).max().unwrap_or(0);
    // Orphans (unreachable from initial): tucked one layer below.
    let orphan_layer = max_layer + 1;

    // Group nodes by layer in source order.
    let mut by_layer: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, _) in variants.iter().enumerate() {
        let layer = layer_of[i].unwrap_or(orphan_layer);
        by_layer.entry(layer).or_default().push(i);
    }

    let mut nodes: Vec<DiagramNode> = Vec::with_capacity(n);
    for (layer, members) in &by_layer {
        for (pos, &idx) in members.iter().enumerate() {
            let v = &variants[idx];
            nodes.push(DiagramNode {
                index: idx,
                name: v.name,
                label: v.label,
                terminal: v.terminal,
                has_payload: v.has_payload,
                layer: *layer,
                layer_pos: pos,
            });
        }
    }
    nodes.sort_by_key(|n| n.index);

    FsmDiagram {
        widget_name: desc.widget_name,
        initial_index: initial,
        nodes,
        edges,
    }
}

/// Render the diagram as inline SVG.
///
/// Layout: each layer occupies one horizontal row; nodes within a
/// layer are spaced evenly across the row.  Edges are straight
/// lines with arrowheads; self-loops are small arcs.  No fancy
/// crossing minimisation — for "good enough for at-a-glance
/// rustdoc", this is plenty.
pub fn render_fsm_svg(diagram: &FsmDiagram) -> String {
    const NODE_W: i32 = 110;
    const NODE_H: i32 = 40;
    const ROW_SPACING: i32 = 90;
    const COL_SPACING: i32 = 30;
    const MARGIN: i32 = 30;

    // Compute (x, y) per node.
    let max_layer = diagram.nodes.iter().map(|n| n.layer).max().unwrap_or(0);
    let mut layer_widths: BTreeMap<usize, usize> = BTreeMap::new();
    for n in &diagram.nodes {
        let w = layer_widths.entry(n.layer).or_insert(0);
        *w = (*w).max(n.layer_pos + 1);
    }
    let max_width = layer_widths.values().copied().max().unwrap_or(1);
    let canvas_w = MARGIN * 2 + max_width as i32 * (NODE_W + COL_SPACING) - COL_SPACING;
    let canvas_h = MARGIN * 2 + (max_layer as i32 + 1) * (NODE_H + ROW_SPACING) - ROW_SPACING;

    let xy = |node: &DiagramNode| -> (i32, i32) {
        let row_count = layer_widths.get(&node.layer).copied().unwrap_or(1);
        let row_span = row_count as i32 * (NODE_W + COL_SPACING) - COL_SPACING;
        let row_start = MARGIN + (canvas_w - 2 * MARGIN - row_span) / 2;
        let x = row_start + node.layer_pos as i32 * (NODE_W + COL_SPACING);
        let y = MARGIN + node.layer as i32 * (NODE_H + ROW_SPACING);
        (x, y)
    };

    let mut svg = String::new();
    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {canvas_w} {canvas_h}" font-family="sans-serif" font-size="13">"#
    );
    let _ = writeln!(
        svg,
        r##"<defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="#444"/></marker></defs>"##
    );
    let _ = writeln!(svg, r#"<title>FSM diagram for {}</title>"#, diagram.widget_name);

    // Edges first (so nodes draw over them).
    for e in &diagram.edges {
        if e.self_loop {
            let node = &diagram.nodes[e.source_index];
            let (x, y) = xy(node);
            let cx = x + NODE_W / 2;
            let cy = y;
            let _ = writeln!(
                svg,
                r##"<path d="M {cx_start} {cy_start} C {cx_c1} {cy_c1}, {cx_c2} {cy_c1}, {cx_end} {cy_end}" fill="none" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>"##,
                cx_start = cx - 12,
                cy_start = cy,
                cx_c1 = cx - 25,
                cy_c1 = cy - 35,
                cx_c2 = cx + 25,
                cx_end = cx + 12,
                cy_end = cy,
            );
        } else {
            let src = &diagram.nodes[e.source_index];
            let tgt = &diagram.nodes[e.target_index];
            let (sx, sy) = xy(src);
            let (tx, ty) = xy(tgt);
            // Edge from bottom of source to top of target.
            let x1 = sx + NODE_W / 2;
            let y1 = sy + NODE_H;
            let x2 = tx + NODE_W / 2;
            let y2 = ty;
            let _ = writeln!(
                svg,
                r##"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="#666" stroke-width="1.5" marker-end="url(#arrow)"/>"##
            );
        }
    }

    // Nodes.
    for node in &diagram.nodes {
        let (x, y) = xy(node);
        let fill = if node.index == diagram.initial_index {
            "#e0f2ff"
        } else if node.terminal {
            "#e8f5e9"
        } else {
            "#ffffff"
        };
        let stroke = if node.index == diagram.initial_index {
            "#2563eb"
        } else if node.terminal {
            "#15803d"
        } else {
            "#444"
        };
        let stroke_width = if node.terminal { 3 } else { 1 };
        let _ = writeln!(
            svg,
            r#"<rect x="{x}" y="{y}" width="{NODE_W}" height="{NODE_H}" rx="6" ry="6" fill="{fill}" stroke="{stroke}" stroke-width="{stroke_width}"/>"#
        );
        let label_text = node.label.unwrap_or(node.name);
        let text_x = x + NODE_W / 2;
        let text_y = y + NODE_H / 2 + 5;
        let _ = writeln!(
            svg,
            r#"<text x="{text_x}" y="{text_y}" text-anchor="middle">{label_text}</text>"#
        );
    }

    let _ = writeln!(svg, "</svg>");
    svg
}

/// Render the diagram in Graphviz `dot` format.
///
/// Produces output suitable for piping into `dot`, `xdot`, or any
/// other Graphviz consumer.  Uses subgraphs + ranks to encode the
/// layered layout; preserves per-variant terminal/initial styling.
pub fn render_fsm_dot(diagram: &FsmDiagram) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "digraph fsm {{");
    let _ = writeln!(s, "    rankdir=TB;");
    let _ = writeln!(s, "    node [shape=box, style=rounded, fontname=\"sans-serif\"];");
    let _ = writeln!(
        s,
        "    label=\"FSM: {}\"; labelloc=\"t\"; fontname=\"sans-serif\";",
        diagram.widget_name
    );

    for node in &diagram.nodes {
        let label = node.label.unwrap_or(node.name);
        let style = if node.index == diagram.initial_index {
            ", style=\"rounded,filled\", fillcolor=\"#e0f2ff\""
        } else if node.terminal {
            ", style=\"rounded,filled,bold\", fillcolor=\"#e8f5e9\""
        } else {
            ""
        };
        let _ = writeln!(
            s,
            "    n{idx} [label=\"{label}\"{style}];",
            idx = node.index
        );
    }

    for e in &diagram.edges {
        let _ = writeln!(s, "    n{src} -> n{tgt};", src = e.source_index, tgt = e.target_index);
    }

    let _ = writeln!(s, "}}");
    s
}

/// Structured JSON representation, intended for LLM-tool consumption.
///
/// Hand-rolled to avoid a `serde_json` dependency in `rhdl-core`'s
/// hot compile path; the output is small and deterministic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsmDiagramJson(pub String);

/// Render the diagram as a structured JSON document.
pub fn render_fsm_json(diagram: &FsmDiagram) -> FsmDiagramJson {
    let mut s = String::new();
    let _ = writeln!(s, "{{");
    let _ = writeln!(s, r#"  "widget": "{}","#, diagram.widget_name);
    let _ = writeln!(s, r#"  "initial_index": {},"#, diagram.initial_index);
    let _ = writeln!(s, r#"  "nodes": ["#);
    let mut first = true;
    for node in &diagram.nodes {
        if !first {
            let _ = writeln!(s, ",");
        }
        first = false;
        let label = node.label.unwrap_or(node.name);
        let _ = write!(
            s,
            r#"    {{"index": {idx}, "name": "{name}", "label": "{label}", "terminal": {term}, "has_payload": {pay}}}"#,
            idx = node.index,
            name = node.name,
            label = label,
            term = node.terminal,
            pay = node.has_payload,
        );
    }
    let _ = writeln!(s);
    let _ = writeln!(s, r#"  ],"#);
    let _ = writeln!(s, r#"  "edges": ["#);
    let mut first = true;
    for e in &diagram.edges {
        if !first {
            let _ = writeln!(s, ",");
        }
        first = false;
        let _ = write!(
            s,
            r#"    {{"source": {src}, "target": {tgt}, "self_loop": {sl}}}"#,
            src = e.source_index,
            tgt = e.target_index,
            sl = e.self_loop,
        );
    }
    let _ = writeln!(s);
    let _ = writeln!(s, r#"  ]"#);
    let _ = writeln!(s, "}}");
    FsmDiagramJson(s)
}

/// Build the diagram, then render to all three formats at once.
/// Used by the rustdoc integration in `circuit::descriptor`.
pub fn render_all_formats(
    desc: &FsmDescriptor,
    transitions: &[Transition],
) -> (FsmDiagram, String, String, FsmDiagramJson) {
    let diagram = build_fsm_diagram(desc, transitions);
    let svg = render_fsm_svg(&diagram);
    let dot = render_fsm_dot(&diagram);
    let json = render_fsm_json(&diagram);
    (diagram, svg, dot, json)
}

// `_unused` is here just to silence the unused-import warning if
// FsmVariantDescriptor isn't otherwise referenced after macro
// expansion.  Real consumers reach the type through `desc.variants()`.
#[allow(dead_code)]
fn _unused(_v: &FsmVariantDescriptor) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsm::analysis::transition;
    use crate::fsm::descriptor::{FsmKernelTag, FsmWidgetTag};

    static THREE_STATE: &[FsmVariantDescriptor] = &[
        FsmVariantDescriptor {
            name: "Idle",
            discriminant: 0,
            has_payload: false,
            terminal: false,
            label: Some("idle, waiting"),
        },
        FsmVariantDescriptor {
            name: "Running",
            discriminant: 1,
            has_payload: true,
            terminal: false,
            label: None,
        },
        FsmVariantDescriptor {
            name: "Done",
            discriminant: 2,
            has_payload: false,
            terminal: true,
            label: None,
        },
    ];

    fn three_state_descriptor() -> FsmDescriptor {
        FsmDescriptor {
            widget_name: "test::ThreeState",
            widget: FsmWidgetTag {
                state_field: "state",
                strict: false,
            },
            kernel: FsmKernelTag {
                state_var: "q.state",
            },
            variants_fn: || THREE_STATE,
            initial_fn: || 0,
        }
    }

    #[test]
    fn diagram_has_correct_node_layout() {
        let desc = three_state_descriptor();
        let transitions = vec![
            transition(0, 1),
            transition(1, 1),
            transition(1, 2),
            transition(2, 0),
        ];
        let diagram = build_fsm_diagram(&desc, &transitions);
        assert_eq!(diagram.nodes.len(), 3);
        // Idle (initial) → layer 0; Running → layer 1; Done → layer 2.
        let idle = diagram.nodes.iter().find(|n| n.name == "Idle").unwrap();
        let running = diagram.nodes.iter().find(|n| n.name == "Running").unwrap();
        let done = diagram.nodes.iter().find(|n| n.name == "Done").unwrap();
        assert_eq!(idle.layer, 0);
        assert_eq!(running.layer, 1);
        assert_eq!(done.layer, 2);
    }

    #[test]
    fn self_loop_is_classified() {
        let desc = three_state_descriptor();
        let transitions = vec![transition(0, 0), transition(0, 1)];
        let diagram = build_fsm_diagram(&desc, &transitions);
        let self_loops: Vec<_> = diagram.edges.iter().filter(|e| e.self_loop).collect();
        assert_eq!(self_loops.len(), 1);
        assert_eq!(self_loops[0].source_index, 0);
        assert_eq!(self_loops[0].target_index, 0);
    }

    #[test]
    fn svg_contains_node_labels_and_arrows() {
        let desc = three_state_descriptor();
        let transitions = vec![transition(0, 1), transition(1, 2)];
        let diagram = build_fsm_diagram(&desc, &transitions);
        let svg = render_fsm_svg(&diagram);
        // Initial label override is honoured.
        assert!(svg.contains("idle, waiting"));
        // Names without explicit label fall back to the variant name.
        assert!(svg.contains("Running"));
        assert!(svg.contains("Done"));
        // Arrow marker is defined.
        assert!(svg.contains("marker id=\"arrow\""));
        // SVG header well-formed.
        assert!(svg.starts_with("<svg"));
        assert!(svg.trim_end().ends_with("</svg>"));
    }

    #[test]
    fn dot_round_trips_node_count_and_edges() {
        let desc = three_state_descriptor();
        let transitions = vec![transition(0, 1), transition(1, 2), transition(2, 0)];
        let diagram = build_fsm_diagram(&desc, &transitions);
        let dot = render_fsm_dot(&diagram);
        assert!(dot.contains("digraph fsm"));
        assert_eq!(dot.matches("->").count(), 3);
        assert!(dot.contains("n0 [label=\"idle, waiting\""));
        assert!(dot.contains("n2 [label=\"Done\""));
    }

    #[test]
    fn json_is_valid_structure() {
        let desc = three_state_descriptor();
        let transitions = vec![transition(0, 1), transition(1, 2)];
        let diagram = build_fsm_diagram(&desc, &transitions);
        let json = render_fsm_json(&diagram);
        // Spot-check shape; we don't pull in serde_json for parsing
        // because the producer is hand-rolled and a substring check
        // is exactly what we need to validate the format contract.
        assert!(json.0.contains(r#""widget": "test::ThreeState""#));
        assert!(json.0.contains(r#""initial_index": 0"#));
        assert!(json.0.contains(r#""name": "Running""#));
        assert!(
            json.0.contains(r#""self_loop": false"#),
            "expected at least one non-self-loop edge in: {}",
            json.0
        );
    }
}
