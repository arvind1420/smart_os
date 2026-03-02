/// A/B partition management for Smart OS.
///
/// Maintains two system partitions (A and B). Only one is active at a time.
/// Updates are applied to the standby partition via binary deltas, then an
/// atomic swap makes it active. Rollback is always possible by swapping back.

use alloc::string::String;
use spin::Mutex;
use smartpack::Value;

// ── Partition State ─────────────────────────────────────────────────

/// State of a system partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionState {
    /// Currently booted and serving the OS.
    Active,
    /// Available for receiving updates.
    Standby,
    /// Corrupted or never initialized.
    Invalid,
    /// Currently receiving an update.
    Updating,
}

impl PartitionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            PartitionState::Active => "active",
            PartitionState::Standby => "standby",
            PartitionState::Invalid => "invalid",
            PartitionState::Updating => "updating",
        }
    }
}

// ── System Partition ────────────────────────────────────────────────

/// Metadata for a system partition.
#[derive(Debug, Clone)]
pub struct SystemPartition {
    /// Partition identifier (0=A, 1=B).
    pub id: u8,
    /// Version string for the OS on this partition.
    pub version: String,
    /// Current state of this partition.
    pub state: PartitionState,
    /// Start sector on disk (if persisted).
    pub sector_start: u64,
    /// Number of sectors allocated.
    pub sector_count: u64,
    /// Number of successful boots from this partition.
    pub boot_count: u32,
    /// CRC32 checksum of partition content.
    pub checksum: u32,
}

// ── Partition Manager ───────────────────────────────────────────────

/// Manages the A/B partition pair.
pub struct PartitionManager {
    /// The two system partitions [A, B].
    pub partitions: [SystemPartition; 2],
    /// Index of the currently active partition (0 or 1).
    pub active_idx: usize,
}

/// Global partition manager.
pub static PARTITIONS: Mutex<Option<PartitionManager>> = Mutex::new(None);

impl PartitionManager {
    /// Create a new partition manager with default (initial boot) state.
    pub fn new() -> Self {
        Self {
            partitions: [
                SystemPartition {
                    id: 0,
                    version: String::from("0.7.0"),
                    state: PartitionState::Active,
                    sector_start: 0,
                    sector_count: 4096, // 2 MiB default
                    boot_count: 1,
                    checksum: 0,
                },
                SystemPartition {
                    id: 1,
                    version: String::from("0.7.0"),
                    state: PartitionState::Standby,
                    sector_start: 4096,
                    sector_count: 4096,
                    boot_count: 0,
                    checksum: 0,
                },
            ],
            active_idx: 0,
        }
    }

    /// Get the currently active partition.
    pub fn active(&self) -> &SystemPartition {
        &self.partitions[self.active_idx]
    }

    /// Get the standby partition.
    pub fn standby(&self) -> &SystemPartition {
        &self.partitions[1 - self.active_idx]
    }

    /// Begin an update: mark standby partition as Updating.
    pub fn begin_update(&mut self) -> Result<(), &'static str> {
        let idx = 1 - self.active_idx;
        if self.partitions[idx].state == PartitionState::Updating {
            return Err("Standby partition already being updated");
        }
        self.partitions[idx].state = PartitionState::Updating;
        crate::serial_println!(
            "[partition] Partition {} entering update state.",
            self.partitions[idx].id,
        );
        Ok(())
    }

    /// Apply a binary delta update to the standby partition.
    ///
    /// The delta format is a simple sequence of:
    ///   [sector_offset: u64] [data_length: u32] [data: u8*]
    ///
    /// Each entry patches the standby partition at the given sector offset.
    pub fn apply_delta(&mut self, delta: &[u8]) -> Result<(), &'static str> {
        let idx = 1 - self.active_idx;
        if self.partitions[idx].state != PartitionState::Updating {
            return Err("Standby partition not in update state");
        }

        // Parse and apply delta entries
        let mut pos = 0;
        let mut patches = 0u32;
        while pos + 12 <= delta.len() {
            // Read sector offset (8 bytes, little-endian)
            let sector_offset = u64::from_le_bytes([
                delta[pos],
                delta[pos + 1],
                delta[pos + 2],
                delta[pos + 3],
                delta[pos + 4],
                delta[pos + 5],
                delta[pos + 6],
                delta[pos + 7],
            ]);
            pos += 8;

            // Read data length (4 bytes, little-endian)
            let data_len = u32::from_le_bytes([
                delta[pos],
                delta[pos + 1],
                delta[pos + 2],
                delta[pos + 3],
            ]) as usize;
            pos += 4;

            if pos + data_len > delta.len() {
                return Err("Delta data truncated");
            }

            // The actual disk write would use:
            // let target_sector = self.partitions[idx].sector_start + sector_offset;
            // crate::drivers::virtio_blk::write_sector(target_sector, &delta[pos..pos+data_len])?;
            // For now, we log the patch intent:
            let _target_sector = self.partitions[idx].sector_start + sector_offset;
            patches += 1;

            pos += data_len;
        }

        // Mark update as complete
        self.partitions[idx].state = PartitionState::Standby;
        crate::serial_println!(
            "[partition] Delta applied: {} patches to partition {}.",
            patches,
            self.partitions[idx].id,
        );
        Ok(())
    }

    /// Atomically swap active and standby partitions.
    pub fn swap_active(&mut self) -> Result<(), &'static str> {
        let new_active = 1 - self.active_idx;
        if self.partitions[new_active].state == PartitionState::Invalid {
            return Err("Cannot activate invalid partition");
        }
        if self.partitions[new_active].state == PartitionState::Updating {
            return Err("Cannot activate partition that is being updated");
        }

        // Swap states
        self.partitions[self.active_idx].state = PartitionState::Standby;
        self.partitions[new_active].state = PartitionState::Active;
        self.partitions[new_active].boot_count += 1;
        self.active_idx = new_active;

        crate::serial_println!(
            "[partition] Swapped: partition {} now active (boot #{}).",
            self.partitions[new_active].id,
            self.partitions[new_active].boot_count,
        );
        Ok(())
    }

    /// Rollback: swap back to previous active partition.
    pub fn rollback(&mut self) -> Result<(), &'static str> {
        crate::serial_println!("[partition] Rolling back to previous partition...");
        self.swap_active()
    }
}

// ── Initialization ──────────────────────────────────────────────────

/// Initialize the partition manager.
pub fn init() {
    let manager = PartitionManager::new();
    crate::serial_println!(
        "[partition] A/B partitions: A={} ({}), B={} ({}), active={}",
        manager.partitions[0].version,
        manager.partitions[0].state.as_str(),
        manager.partitions[1].version,
        manager.partitions[1].state.as_str(),
        if manager.active_idx == 0 { "A" } else { "B" },
    );
    *PARTITIONS.lock() = Some(manager);
}

/// Get partition status as a SmartPack Value (for system monitor / terminal).
pub fn partition_info() -> Value {
    let mgr = PARTITIONS.lock();
    match mgr.as_ref() {
        Some(m) => Value::Map(alloc::vec![
            (
                Value::String(String::from("active")),
                Value::String(if m.active_idx == 0 {
                    String::from("A")
                } else {
                    String::from("B")
                }),
            ),
            (
                Value::String(String::from("partition_a")),
                partition_to_value(&m.partitions[0]),
            ),
            (
                Value::String(String::from("partition_b")),
                partition_to_value(&m.partitions[1]),
            ),
        ]),
        None => Value::Null,
    }
}

/// Convert a partition to SmartPack Value.
fn partition_to_value(p: &SystemPartition) -> Value {
    Value::Map(alloc::vec![
        (
            Value::String(String::from("version")),
            Value::String(p.version.clone()),
        ),
        (
            Value::String(String::from("state")),
            Value::String(String::from(p.state.as_str())),
        ),
        (
            Value::String(String::from("boot_count")),
            Value::UInt32(p.boot_count),
        ),
    ])
}
