/// Smart OS Image Builder
///
/// Creates a single 1GB bootable image for UEFI.

use std::path::{Path, PathBuf};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write, Seek, SeekFrom};

fn main() {
    let kernel_path = find_kernel_binary();
    println!("Smart OS Enterprise Image Builder");
    println!("=================================");

    let out_img = output_path("smartos.img");
    let data_img = output_path("smartos-data.img");
    
    // 1. Create a standard bootable disk image using the bootloader crate
    println!("Step 1: Generating base bootable image...");
    bootloader::UefiBoot::new(&kernel_path)
        .create_disk_image(&out_img)
        .expect("Failed to create base boot image");

    // 2. Create and Format a SEPARATE data image (512MB)
    println!("Step 2: Creating and Formatting DATA partition image...");
    let data_size = 512 * 1024 * 1024;
    {
        let file = File::create(&data_img).unwrap();
        file.set_len(data_size).unwrap();
        
        let mut file = OpenOptions::new().read(true).write(true).open(&data_img).unwrap();
        let format_opts = fatfs::FormatVolumeOptions::new()
            .volume_label(*b"SMARTDATA  ")
            .fat_type(fatfs::FatType::Fat32);
            
        fatfs::format_volume(&mut file, format_opts).unwrap();
        
        let fs = fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new()).unwrap();
        let bin_dir = fs.root_dir().create_dir("bin").unwrap();

        let apps = ["browser", "games", "pkg_installer", "dashboard", "shell", "office", "spreadsheet", "presentation", "crm"];
        let app_bin_dir = get_workspace_root().join("target/x86_64-unknown-none/release");

        for app_name in apps {
            let app_path = app_bin_dir.join(app_name);
            if app_path.exists() {
                println!("  Adding /bin/{}...", app_name);
                let mut app_file = File::open(&app_path).unwrap();
                let mut out_file = bin_dir.create_file(app_name).unwrap();
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = app_file.read(&mut buf).unwrap();
                    if n == 0 { break; }
                    out_file.write_all(&buf[..n]).unwrap();
                }
            }
        }
    }

    // 3. Expand the main image and merge the data image
    println!("Step 3: Merging images and updating partition table...");
    let boot_size = File::open(&out_img).unwrap().metadata().unwrap().len();
    // Align boot size to 1MB
    let boot_size_aligned = (boot_size + 0xFFFFF) & !0xFFFFF;
    let total_size = boot_size_aligned + data_size;
    
    // Growth
    {
        let file = OpenOptions::new().read(true).write(true).open(&out_img).unwrap();
        file.set_len(total_size).unwrap();
    }

    // UPDATING MBR (Works even if it's a GPT protective MBR)
    {
        let mut file = OpenOptions::new().read(true).write(true).open(&out_img).unwrap();
        let mut mbr = mbrman::MBR::read_from(&mut file, 512).unwrap();
        
        // Find existing partition (ESP) or GPT protective entry
        let start_lba = (boot_size_aligned / 512) as u32;
        let sectors = (data_size / 512) as u32;
        
        // Add DATA partition to the first available slot
        let free_idx = mbr.iter().find(|(_, p)| p.is_unused()).map(|(i, _)| i).unwrap_or(1);

        mbr[free_idx] = mbrman::MBRPartitionEntry {
            boot: mbrman::BOOT_INACTIVE,
            first_chs: mbrman::CHS::empty(),
            sys: 0x0C,
            last_chs: mbrman::CHS::empty(),
            starting_lba: start_lba,
            sectors,
        };
        
        file.seek(SeekFrom::Start(0)).unwrap();
        mbr.write_into(&mut file).unwrap();
    }

    // Write the data image at the aligned offset
    {
        let mut file = OpenOptions::new().read(true).write(true).open(&out_img).unwrap();
        let mut data_file = File::open(&data_img).unwrap();
        file.seek(SeekFrom::Start(boot_size_aligned)).unwrap();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = data_file.read(&mut buf).unwrap();
            if n == 0 { break; }
            file.write_all(&buf[..n]).unwrap();
        }
    }

    println!("\nSUCCESS!");
    println!("Flashable Image: {}", out_img.display());
}

fn find_kernel_binary() -> PathBuf {
    let workspace_root = get_workspace_root();
    let candidates = [
        workspace_root.join("target/x86_64-unknown-none/release/smartos-kernel"),
        workspace_root.join("target/x86_64-unknown-none/debug/smartos-kernel"),
    ];
    for path in &candidates { if path.exists() { return path.clone(); } }
    panic!("Kernel not found");
}

fn get_workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn output_path(name: &str) -> PathBuf {
    let out_dir = get_workspace_root().join("target");
    std::fs::create_dir_all(&out_dir).ok();
    out_dir.join(name)
}
