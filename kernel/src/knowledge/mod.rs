/// Knowledge Graph — Unified Data API for Smart OS.
///
/// Provides a system-wide knowledge graph backed by SmartPack Values.
/// Instead of every app having its own isolated database, apps use this
/// kernel service to store and query structured data, enabling deep
/// cross-app integration.

pub mod graph;
pub mod query;

/// Initialize the knowledge graph with system seed data.
pub fn init() {
    graph::init();
    crate::serial_println!(
        "[knowledge] Knowledge graph initialized ({} nodes, {} edges).",
        graph::node_count(),
        graph::edge_count(),
    );
}
