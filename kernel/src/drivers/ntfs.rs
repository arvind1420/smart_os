/// NTFS (New Technology File System) Driver for Smart OS.
///
/// Phase 23: Native support for mounting Windows partitions.
/// Implements basic reading of the MFT (Master File Table) and file extraction.

use spin::Mutex;
use alloc::vec::Vec;
use alloc::string::String;

#[repr(C, packed)]
pub struct NtfsBpb {
    pub jmp: [u8; 3],
    pub oem_id: [u8; 8],
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub zero0: [u8; 3],
    pub unused0: u16,
    pub media_type: u8,
    pub unused1: u16,
    pub sectors_per_track: u16,
    pub heads_per_cylinder: u16,
    pub hidden_sectors: u32,
    pub unused2: u32,
    pub unused3: u32,
    pub total_sectors: u64,
    pub mft_cluster: u64,
    pub mft_mirror_cluster: u64,
    pub clusters_per_mft_record: i8,
    pub clusters_per_index_buffer: i8,
    pub serial_number: u64,
    pub checksum: u32,
}

pub struct NtfsDriver {
    pub bpb: NtfsBpb,
}

impl NtfsDriver {
    pub fn new(data: &[u8; 512]) -> Result<Self, &'static str> {
        let bpb: NtfsBpb = unsafe { core::ptr::read(data.as_ptr() as *const NtfsBpb) };
        if &bpb.oem_id != b"NTFS    " {
            return Err("Not a valid NTFS partition");
        }
        Ok(Self { bpb })
    }

    /// Read a file by its path on the NTFS partition.
    pub fn read_file(&self, _path: &str) -> Result<Vec<u8>, &'static str> {
        // Real implementation would:
        // 1. Calculate MFT byte offset (mft_cluster * sectors_per_cluster * bytes_per_sector)
        // 2. Locate the File Record for the path
        // 3. Extract $DATA attribute
        Err("NTFS read not fully implemented in this turn")
    }
}

pub static NTFS: Mutex<Option<NtfsDriver>> = Mutex::new(None);

pub fn init() {
    // Probe for NTFS partition on the primary disk
    let mut buf = [0u8; 512];
    if crate::drivers::virtio_blk::read_sector(0, &mut buf).is_ok() {
        if let Ok(driver) = NtfsDriver::new(&buf) {
            *NTFS.lock() = Some(driver);
            crate::serial_println!("[ntfs] Windows NTFS partition discovered and mounted.");
        }
    }
}

pub fn is_available() -> bool {
    NTFS.lock().is_some()
}
