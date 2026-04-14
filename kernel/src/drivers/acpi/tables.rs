/// ACPI Table Parsing for Smart OS.
///
/// Provides structures and functions to locate and parse ACPI tables
/// (MADT, HPET, MCFG, etc.) starting from the RSDP.

use core::mem::size_of;

/// Root System Description Pointer (RSDP) - ACPI 1.0
#[repr(C, packed)]
pub struct Rsdp {
    pub signature: [u8; 8],
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub revision: u8,
    pub rsdt_address: u32,
}

/// Extended Root System Description Pointer (RSDP) - ACPI 2.0+
#[repr(C, packed)]
pub struct RsdpExtended {
    pub base: Rsdp,
    pub length: u32,
    pub xsdt_address: u64,
    pub extended_checksum: u8,
    pub reserved: [u8; 3],
}

/// Standard ACPI Table Header.
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct SdtHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub oem_revision: u32,
    pub creator_id: [u32; 1],
    pub creator_revision: u32,
}

impl SdtHeader {
    pub fn signature_str(&self) -> &str {
        core::str::from_utf8(&self.signature).unwrap_or("????")
    }
}

/// Fixed ACPI Description Table (FADT).
#[repr(C, packed)]
pub struct Fadt {
    pub header: SdtHeader,
    pub firmware_ctrl: u32,
    pub dsdt: u32,
    pub reserved: u8,
    pub preferred_pm_profile: u8,
    pub sci_int: u16,
    pub smi_cmd: u32,
    pub acpi_enable: u8,
    pub acpi_disable: u8,
    pub s4bios_req: u8,
    pub pstate_cnt: u8,
    pub pm1a_evt_blk: u32,
    pub pm1b_evt_blk: u32,
    pub pm1a_cnt_blk: u32,
    pub pm1b_cnt_blk: u32,
    pub pm2_cnt_blk: u32,
    pub pm_tmr_blk: u32,
    pub gpe0_blk: u32,
    pub gpe1_blk: u32,
    pub pm1_evt_len: u8,
    pub pm1_cnt_len: u8,
    pub pm2_cnt_len: u8,
    pub pm_tmr_len: u8,
    pub gpe0_blk_len: u8,
    pub gpe1_blk_len: u8,
    pub gpe1_base: u8,
    pub cst_cnt: u8,
    pub p_lvl2_lat: u16,
    pub p_lvl3_lat: u16,
    pub flush_size: u16,
    pub flush_stride: u16,
    pub duty_offset: u8,
    pub duty_width: u8,
    pub day_alrm: u8,
    pub mon_alrm: u8,
    pub century: u8,
    pub iapc_boot_arch: u16,
    pub reserved2: u8,
    pub flags: u32,
    pub reset_reg: GenericAddressStructure,
    pub reset_value: u8,
    pub reserved3: [u8; 3],
    pub x_firmware_ctrl: u64,
    pub x_dsdt: u64,
}

/// Multiple APIC Description Table (MADT).
#[repr(C, packed)]
pub struct Madt {
    pub header: SdtHeader,
    pub local_apic_address: u32,
    pub flags: u32,
}

/// MADT Entry Header.
#[repr(C, packed)]
pub struct MadtEntryHeader {
    pub entry_type: u8,
    pub length: u8,
}

/// MADT Entry: Local APIC.
#[repr(C, packed)]
pub struct MadtLocalApic {
    pub header: MadtEntryHeader,
    pub processor_id: u8,
    pub apic_id: u8,
    pub flags: u32,
}

/// MADT Entry: I/O APIC.
#[repr(C, packed)]
pub struct MadtIoApic {
    pub header: MadtEntryHeader,
    pub io_apic_id: u8,
    pub reserved: u8,
    pub io_apic_address: u32,
    pub global_system_interrupt_base: u32,
}

/// HPET Table.
#[repr(C, packed)]
pub struct HpetTable {
    pub header: SdtHeader,
    pub event_timer_block_id: u32,
    pub base_address: GenericAddressStructure,
    pub hpet_number: u8,
    pub minimum_tick: u16,
    pub page_protection: u8,
}

#[repr(C, packed)]
pub struct GenericAddressStructure {
    pub address_space: u8,
    pub register_bit_width: u8,
    pub register_bit_offset: u8,
    pub access_size: u8,
    pub address: u64,
}

/// ACPI Table Manager.
pub struct AcpiTables {
    rsdp_addr: u64,
}

impl AcpiTables {
    pub fn new(rsdp_addr: u64) -> Self {
        Self { rsdp_addr }
    }

    /// Find a table by its 4-character signature.
    pub fn find_table(&self, signature: &[u8; 4]) -> Option<u64> {
        let phys_offset = crate::memory::paging::phys_offset().as_u64();
        let rsdp = unsafe { &*( (phys_offset + self.rsdp_addr) as *const Rsdp ) };
        
        if rsdp.revision >= 2 {
            let rsdp_ext = unsafe { &*( (phys_offset + self.rsdp_addr) as *const RsdpExtended ) };
            self.find_in_xsdt(rsdp_ext.xsdt_address, signature)
        } else {
            self.find_in_rsdt(rsdp.rsdt_address as u64, signature)
        }
    }

    fn find_in_rsdt(&self, rsdt_phys: u64, signature: &[u8; 4]) -> Option<u64> {
        let phys_offset = crate::memory::paging::phys_offset().as_u64();
        let header = unsafe { &*( (phys_offset + rsdt_phys) as *const SdtHeader ) };
        let entries = (header.length as usize - size_of::<SdtHeader>()) / 4;
        let entry_ptr = (phys_offset + rsdt_phys + size_of::<SdtHeader>() as u64) as *const u32;

        for i in 0..entries {
            let table_phys = unsafe { *entry_ptr.add(i) } as u64;
            let table_header = unsafe { &*( (phys_offset + table_phys) as *const SdtHeader ) };
            if &table_header.signature == signature {
                return Some(table_phys);
            }
        }
        None
    }

    fn find_in_xsdt(&self, xsdt_phys: u64, signature: &[u8; 4]) -> Option<u64> {
        let phys_offset = crate::memory::paging::phys_offset().as_u64();
        let header = unsafe { &*( (phys_offset + xsdt_phys) as *const SdtHeader ) };
        let entries = (header.length as usize - size_of::<SdtHeader>()) / 8;
        let entry_ptr = (phys_offset + xsdt_phys + size_of::<SdtHeader>() as u64) as *const u64;

        for i in 0..entries {
            let table_phys = unsafe { *entry_ptr.add(i) };
            let table_header = unsafe { &*( (phys_offset + table_phys) as *const SdtHeader ) };
            if &table_header.signature == signature {
                return Some(table_phys);
            }
        }
        None
    }

    /// Parse MADT to find LAPIC IDs.
    pub fn get_lapic_ids(&self) -> Vec<u8> {
        let mut ids = Vec::new();
        let phys_offset = crate::memory::paging::phys_offset().as_u64();
        
        if let Some(madt_phys) = self.find_table(b"APIC") {
            let madt = unsafe { &*( (phys_offset + madt_phys) as *const Madt ) };
            let mut curr = madt_phys + size_of::<Madt>() as u64;
            let end = madt_phys + madt.header.length as u64;

            while curr < end {
                let header = unsafe { &*( (phys_offset + curr) as *const MadtEntryHeader ) };
                if header.length == 0 { break; }
                
                if header.entry_type == 0 { // Local APIC
                    let lapic = unsafe { &*( (phys_offset + curr) as *const MadtLocalApic ) };
                    if lapic.flags & 1 != 0 { // Enabled
                        ids.push(lapic.apic_id);
                    }
                }
                curr += header.length as u64;
            }
        }
        ids
    }
}

use alloc::vec::Vec;
