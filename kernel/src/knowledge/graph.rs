/// Core knowledge graph data structure for Smart OS.
///
/// Nodes represent entities (contacts, notes, settings, apps, files).
/// Edges represent typed relationships between nodes.
/// All properties are stored as SmartPack Values for maximum flexibility.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use smartpack::Value;

/// Unique identifier for a node.
pub type NodeId = u64;
/// Unique identifier for an edge.
pub type EdgeId = u64;

// ── Atomic ID generators ────────────────────────────────────────────

static NEXT_NODE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_EDGE_ID: AtomicU64 = AtomicU64::new(1);

fn alloc_node_id() -> NodeId {
    NEXT_NODE_ID.fetch_add(1, Ordering::Relaxed)
}

fn alloc_edge_id() -> EdgeId {
    NEXT_EDGE_ID.fetch_add(1, Ordering::Relaxed)
}

// ── Node and Edge ───────────────────────────────────────────────────

/// A node in the knowledge graph.
#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    /// Type of the node (e.g. "contact", "note", "app", "setting", "file").
    pub node_type: String,
    /// Arbitrary key-value properties stored as SmartPack Map.
    pub properties: Value,
}

/// A directed, typed edge between two nodes.
#[derive(Debug, Clone)]
pub struct Edge {
    pub id: EdgeId,
    pub from: NodeId,
    pub to: NodeId,
    /// Relationship type (e.g. "authored_by", "tagged_with", "depends_on").
    pub relation: String,
    /// Optional edge metadata.
    pub properties: Value,
}

// ── Knowledge Graph ─────────────────────────────────────────────────

/// The in-memory knowledge graph.
pub struct KnowledgeGraph {
    /// All nodes indexed by ID.
    nodes: BTreeMap<NodeId, Node>,
    /// All edges indexed by ID.
    edges: BTreeMap<EdgeId, Edge>,
    /// Index: node_type → Vec<NodeId> for fast type queries.
    type_index: BTreeMap<String, Vec<NodeId>>,
    /// Index: from_node → Vec<EdgeId> for outgoing edge lookups.
    outgoing: BTreeMap<NodeId, Vec<EdgeId>>,
    /// Index: to_node → Vec<EdgeId> for incoming edge lookups.
    incoming: BTreeMap<NodeId, Vec<EdgeId>>,
}

/// Global knowledge graph instance.
pub static GRAPH: Mutex<Option<KnowledgeGraph>> = Mutex::new(None);

impl KnowledgeGraph {
    /// Create a new empty knowledge graph.
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            type_index: BTreeMap::new(),
            outgoing: BTreeMap::new(),
            incoming: BTreeMap::new(),
        }
    }

    /// Insert a node into the graph. Returns its NodeId.
    pub fn insert_node(&mut self, node_type: &str, properties: Value) -> NodeId {
        let id = alloc_node_id();
        let node = Node {
            id,
            node_type: String::from(node_type),
            properties,
        };

        // Update type index
        self.type_index
            .entry(String::from(node_type))
            .or_insert_with(Vec::new)
            .push(id);

        self.nodes.insert(id, node);
        id
    }

    /// Insert a directed edge between two nodes.
    pub fn insert_edge(
        &mut self,
        from: NodeId,
        to: NodeId,
        relation: &str,
        properties: Value,
    ) -> Result<EdgeId, &'static str> {
        // Verify both nodes exist
        if !self.nodes.contains_key(&from) {
            return Err("Source node not found");
        }
        if !self.nodes.contains_key(&to) {
            return Err("Target node not found");
        }

        let id = alloc_edge_id();
        let edge = Edge {
            id,
            from,
            to,
            relation: String::from(relation),
            properties,
        };

        // Update adjacency indices
        self.outgoing
            .entry(from)
            .or_insert_with(Vec::new)
            .push(id);
        self.incoming
            .entry(to)
            .or_insert_with(Vec::new)
            .push(id);

        self.edges.insert(id, edge);
        Ok(id)
    }

    /// Get a node by ID.
    pub fn get_node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// Delete a node and all its connected edges.
    pub fn delete_node(&mut self, id: NodeId) -> Result<(), &'static str> {
        if self.nodes.remove(&id).is_none() {
            return Err("Node not found");
        }

        // Remove from type index
        for ids in self.type_index.values_mut() {
            ids.retain(|&nid| nid != id);
        }

        // Collect edges to remove (outgoing + incoming)
        let mut edges_to_remove = Vec::new();
        if let Some(out_edges) = self.outgoing.remove(&id) {
            edges_to_remove.extend(out_edges);
        }
        if let Some(in_edges) = self.incoming.remove(&id) {
            edges_to_remove.extend(in_edges);
        }

        // Remove the edges and clean up reverse indices
        for eid in &edges_to_remove {
            if let Some(edge) = self.edges.remove(eid) {
                // Clean outgoing for the other end
                if edge.from != id {
                    if let Some(outs) = self.outgoing.get_mut(&edge.from) {
                        outs.retain(|&e| e != *eid);
                    }
                }
                if edge.to != id {
                    if let Some(ins) = self.incoming.get_mut(&edge.to) {
                        ins.retain(|&e| e != *eid);
                    }
                }
            }
        }

        Ok(())
    }

    /// Delete an edge by ID.
    pub fn delete_edge(&mut self, id: EdgeId) -> Result<(), &'static str> {
        let edge = self.edges.remove(&id).ok_or("Edge not found")?;

        if let Some(outs) = self.outgoing.get_mut(&edge.from) {
            outs.retain(|&e| e != id);
        }
        if let Some(ins) = self.incoming.get_mut(&edge.to) {
            ins.retain(|&e| e != id);
        }

        Ok(())
    }

    /// Find all nodes of a given type.
    pub fn find_by_type(&self, node_type: &str) -> Vec<&Node> {
        if let Some(ids) = self.type_index.get(node_type) {
            ids.iter()
                .filter_map(|id| self.nodes.get(id))
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Find all nodes related to a given node (via outgoing edges).
    /// Optionally filter by relation type.
    pub fn find_related(&self, node_id: NodeId, relation: Option<&str>) -> Vec<(&Edge, &Node)> {
        let mut results = Vec::new();

        if let Some(edge_ids) = self.outgoing.get(&node_id) {
            for &eid in edge_ids {
                if let Some(edge) = self.edges.get(&eid) {
                    // Filter by relation if specified
                    if let Some(rel) = relation {
                        if edge.relation.as_str() != rel {
                            continue;
                        }
                    }
                    if let Some(target) = self.nodes.get(&edge.to) {
                        results.push((edge, target));
                    }
                }
            }
        }

        results
    }

    /// Get total node count.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Get total edge count.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Get all distinct node types.
    pub fn node_types(&self) -> Vec<&str> {
        self.type_index
            .keys()
            .filter(|k| {
                self.type_index
                    .get(k.as_str())
                    .map(|ids| !ids.is_empty())
                    .unwrap_or(false)
            })
            .map(|k| k.as_str())
            .collect()
    }

    /// Convert a Node to a SmartPack Value for IPC/syscall responses.
    pub fn node_to_value(&self, node: &Node) -> Value {
        Value::Map(alloc::vec![
            (
                Value::String(String::from("id")),
                Value::UInt64(node.id),
            ),
            (
                Value::String(String::from("type")),
                Value::String(node.node_type.clone()),
            ),
            (
                Value::String(String::from("properties")),
                node.properties.clone(),
            ),
        ])
    }
}

// ── Initialization ──────────────────────────────────────────────────

/// Initialize the knowledge graph with system seed data.
pub fn init() {
    let mut graph = KnowledgeGraph::new();

    // Seed: system info node
    let sys_id = graph.insert_node(
        "system",
        Value::Map(alloc::vec![
            (
                Value::String(String::from("name")),
                Value::String(String::from("Smart OS")),
            ),
            (
                Value::String(String::from("version")),
                Value::String(String::from("0.7.0")),
            ),
            (
                Value::String(String::from("architecture")),
                Value::String(String::from("x86_64")),
            ),
        ]),
    );

    // Seed: kernel subsystem nodes
    let _ai_id = graph.insert_node(
        "subsystem",
        Value::Map(alloc::vec![
            (
                Value::String(String::from("name")),
                Value::String(String::from("AI Inference Engine")),
            ),
            (
                Value::String(String::from("models")),
                Value::UInt32(1),
            ),
        ]),
    );

    let _vfs_id = graph.insert_node(
        "subsystem",
        Value::Map(alloc::vec![
            (
                Value::String(String::from("name")),
                Value::String(String::from("Virtual File System")),
            ),
        ]),
    );

    // Link subsystems to system
    graph.insert_edge(sys_id, _ai_id, "contains", Value::Null).ok();
    graph.insert_edge(sys_id, _vfs_id, "contains", Value::Null).ok();

    *GRAPH.lock() = Some(graph);
}

/// Get node count (for init logging).
pub fn node_count() -> usize {
    GRAPH.lock().as_ref().map(|g| g.node_count()).unwrap_or(0)
}

/// Get edge count (for init logging).
pub fn edge_count() -> usize {
    GRAPH.lock().as_ref().map(|g| g.edge_count()).unwrap_or(0)
}
