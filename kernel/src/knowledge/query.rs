/// Knowledge Graph query API for Smart OS.
///
/// High-level query functions designed to be called from syscall handlers
/// and kernel-internal code. All results are returned as SmartPack Values.

use alloc::string::String;
use alloc::vec::Vec;
use smartpack::Value;
use super::graph::GRAPH;

/// Insert a node into the knowledge graph.
/// Returns the new NodeId as u64.
pub fn kg_insert(node_type: &str, properties: Value) -> Result<u64, &'static str> {
    let mut g = GRAPH.lock();
    let graph = g.as_mut().ok_or("Knowledge graph not initialized")?;
    Ok(graph.insert_node(node_type, properties))
}

/// Query all nodes of a given type.
/// Returns a SmartPack Array of node Values.
pub fn kg_query(node_type: &str) -> Result<Value, &'static str> {
    let g = GRAPH.lock();
    let graph = g.as_ref().ok_or("Knowledge graph not initialized")?;
    let nodes = graph.find_by_type(node_type);
    let array: Vec<Value> = nodes.iter().map(|n| graph.node_to_value(n)).collect();
    Ok(Value::Array(array))
}

/// Link two nodes with a typed relation.
/// Returns the new EdgeId.
pub fn kg_link(
    from: u64,
    to: u64,
    relation: &str,
    properties: Value,
) -> Result<u64, &'static str> {
    let mut g = GRAPH.lock();
    let graph = g.as_mut().ok_or("Knowledge graph not initialized")?;
    graph.insert_edge(from, to, relation, properties)
}

/// Delete a node (and all its edges).
pub fn kg_delete(node_id: u64) -> Result<(), &'static str> {
    let mut g = GRAPH.lock();
    let graph = g.as_mut().ok_or("Knowledge graph not initialized")?;
    graph.delete_node(node_id)
}

/// Find all nodes related to a given node.
/// Returns a SmartPack Array of {node, relation} Maps.
pub fn kg_related(node_id: u64, relation: Option<&str>) -> Result<Value, &'static str> {
    let g = GRAPH.lock();
    let graph = g.as_ref().ok_or("Knowledge graph not initialized")?;
    let related = graph.find_related(node_id, relation);
    let array: Vec<Value> = related
        .iter()
        .map(|(edge, node)| {
            Value::Map(alloc::vec![
                (
                    Value::String(String::from("node")),
                    graph.node_to_value(node),
                ),
                (
                    Value::String(String::from("relation")),
                    Value::String(edge.relation.clone()),
                ),
            ])
        })
        .collect();
    Ok(Value::Array(array))
}

/// Get knowledge graph statistics as a SmartPack Value.
pub fn kg_stats() -> Value {
    let g = GRAPH.lock();
    match g.as_ref() {
        Some(graph) => {
            let types = graph.node_types();
            let type_list: Vec<Value> = types
                .iter()
                .map(|t| Value::String(String::from(*t)))
                .collect();

            Value::Map(alloc::vec![
                (
                    Value::String(String::from("nodes")),
                    Value::UInt64(graph.node_count() as u64),
                ),
                (
                    Value::String(String::from("edges")),
                    Value::UInt64(graph.edge_count() as u64),
                ),
                (
                    Value::String(String::from("types")),
                    Value::Array(type_list),
                ),
            ])
        }
        None => Value::Null,
    }
}
