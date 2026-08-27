use std::collections::HashMap;

use petgraph::graph::NodeIndex;

use crate::{
    common::symtab::RegisterId,
    ntl::{
        Object,
        object::BlackBoxPaths,
        spec::{OpCode, WireKind},
        visit::visit_wires,
    },
};

#[derive(Debug)]
/// A graph representation of the netlist,
/// in which each node represents the source of
/// a register value, and each edge a dependency.
pub struct NetGraph {
    pub reg_map: HashMap<RegisterId<WireKind>, WriteSource>,
    pub graph: petgraph::graph::DiGraph<WriteSource, ()>,
    pub input_node: NodeIndex,
    pub op_nodes: Vec<NodeIndex>,
}

#[derive(Debug, Clone, Copy)]
pub enum WriteSource {
    Input,
    ClockReset,
    OpCode(usize),
}

#[derive(Debug, Clone, Copy)]
pub enum GraphMode {
    Synchronous,
    Asynchronous,
}

fn make_reg_map(input: &Object, mode: GraphMode) -> HashMap<RegisterId<WireKind>, WriteSource> {
    let mut reg_map: HashMap<RegisterId<WireKind>, WriteSource> = HashMap::default();
    // Pass 1
    for (ndx, lop) in input.ops.iter().enumerate() {
        visit_wires(&lop.op, |sense, operand| {
            if let Some(reg) = operand.reg()
                && sense.is_write()
            {
                reg_map.insert(reg, WriteSource::OpCode(ndx));
            }
        });
    }
    match mode {
        GraphMode::Asynchronous => {
            reg_map.extend(
                input
                    .inputs
                    .iter()
                    .flatten()
                    .map(|r| (*r, WriteSource::Input)),
            );
        }
        GraphMode::Synchronous => {
            reg_map.extend(
                input.inputs[0]
                    .iter()
                    .map(|r| (*r, WriteSource::ClockReset)),
            );
            reg_map.extend(
                input
                    .inputs
                    .iter()
                    .skip(1)
                    .flatten()
                    .map(|r| (*r, WriteSource::Input)),
            );
        }
    }
    reg_map
}

/// Add the edges a black box declares, and no others.
///
/// Only the *data* argument is considered: a synchronous black box's `arg`
/// is `[clock_reset, i]` and an asynchronous one's is `[i]`, so the data
/// input is always the last. Clock and reset are excluded for the same
/// reason they are excluded everywhere else in this analysis -- a reset
/// reaches every output by construction, so including it would make the
/// answer uniformly true and useless.
fn add_black_box_edges(
    input: &Object,
    bb: &crate::ntl::spec::BlackBox,
    target: NodeIndex,
    reg_map: &HashMap<RegisterId<WireKind>, WriteSource>,
    input_node: NodeIndex,
    op_nodes: &[NodeIndex],
    graph: &mut petgraph::graph::DiGraph<WriteSource, ()>,
) {
    let Some(decl) = input.black_boxes.get(bb.code.raw()) else {
        // No declaration to consult. Treated as opaque rather than
        // transparent: an absent declaration is the case this whole
        // mechanism exists to stop being answered optimistically.
        connect_all(bb, target, reg_map, input_node, op_nodes, graph);
        return;
    };
    match &decl.paths {
        BlackBoxPaths::None => {}
        BlackBoxPaths::Opaque => connect_all(bb, target, reg_map, input_node, op_nodes, graph),
        BlackBoxPaths::Bits(pairs) => {
            let Some(data) = bb.arg.last() else { return };
            // The op is one graph node, so any declared path makes the
            // whole node depend on that input bit. Per-bit precision
            // lives in the reachability matrix; this graph answers
            // "is there a path at all", which is all its callers ask.
            for (from, _to) in pairs {
                if let Some(reg) = data.get(*from).and_then(|w| w.reg())
                    && let Some(source) = source_node(reg, reg_map, input_node, op_nodes)
                {
                    graph.add_edge(source, target, ());
                }
            }
        }
    }
}

/// Connect every data-input bit of a black box to its node.
fn connect_all(
    bb: &crate::ntl::spec::BlackBox,
    target: NodeIndex,
    reg_map: &HashMap<RegisterId<WireKind>, WriteSource>,
    input_node: NodeIndex,
    op_nodes: &[NodeIndex],
    graph: &mut petgraph::graph::DiGraph<WriteSource, ()>,
) {
    let Some(data) = bb.arg.last() else { return };
    for wire in data {
        if let Some(reg) = wire.reg()
            && let Some(source) = source_node(reg, reg_map, input_node, op_nodes)
        {
            graph.add_edge(source, target, ());
        }
    }
}

/// The graph node a register's value comes from, if it is one this
/// analysis follows. Clock and reset are not.
fn source_node(
    reg: RegisterId<WireKind>,
    reg_map: &HashMap<RegisterId<WireKind>, WriteSource>,
    input_node: NodeIndex,
    op_nodes: &[NodeIndex],
) -> Option<NodeIndex> {
    match reg_map.get(&reg)? {
        WriteSource::Input => Some(input_node),
        WriteSource::OpCode(ndx) => op_nodes.get(*ndx).copied(),
        // Clock and reset are not followed: a reset reaches every output
        // by construction, so an edge from it says nothing.
        WriteSource::ClockReset => None,
    }
}

pub fn make_net_graph(input: &Object, mode: GraphMode) -> NetGraph {
    // Pass 1 - make a map from register to the source of where it is
    // written.
    let reg_map = make_reg_map(input, mode);
    // Pass 2 - make a graph of the write sources.
    let mut graph = petgraph::graph::DiGraph::default();
    // Add a node for the input source
    let input_node = graph.add_node(WriteSource::Input);
    // Add a node for each opcode.
    let op_nodes = (0..input.ops.len())
        .map(|ndx| graph.add_node(WriteSource::OpCode(ndx)))
        .collect::<Vec<_>>();
    // For each opcode, scan the inputs.  For each input,
    // add an edge to the graph from that input's write source to
    // the current opcode
    for (ndx, lop) in input.ops.iter().enumerate() {
        // A black box propagates exactly the paths it declares.
        //
        // This used to `continue` unconditionally, which is how a `DFF`
        // broke a combinational path -- and it was an assumption about
        // the black boxes that happened to exist rather than anything any
        // of them stated. A combinational black box was therefore
        // invisible to every path and loop check in the compiler. See
        // `black-box-connectivity.md`.
        if let OpCode::BlackBox(bb) = &lop.op {
            add_black_box_edges(
                input,
                bb,
                op_nodes[ndx],
                &reg_map,
                input_node,
                &op_nodes,
                &mut graph,
            );
            continue;
        }
        let target = op_nodes[ndx];
        visit_wires(&lop.op, |sense, operand| {
            if let Some(reg) = operand.reg()
                && sense.is_read()
                && let Some(source) = match reg_map[&reg] {
                    WriteSource::Input => Some(input_node),
                    WriteSource::OpCode(ndx) => Some(op_nodes[ndx]),
                    WriteSource::ClockReset => {
                        // For the clock and reset, we don't bother adding edges.
                        None
                    }
                }
            {
                graph.add_edge(source, target, ());
            }
        });
    }

    NetGraph {
        reg_map,
        graph,
        input_node,
        op_nodes,
    }
}
