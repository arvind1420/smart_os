/// Smart OS Image Builder
///
/// Creates bootable UEFI and BIOS disk images from the compiled kernel.
///
/// Usage:
///   cargo build -p smartos-kernel --target x86_64-unknown-none -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem
///   cargo run -p image_builder

use std::path::{Path, PathBuf};

fn main() {
    let kernel_path = find_kernel_binary();
    println!("Smart OS Image Builder");
    println!("======================");
    println!("Kernel binary: {}", kernel_path.display());

    // Create UEFI disk image
    let uefi_img = output_path("smartos-uefi.img");
    bootloader::UefiBoot::new(&kernel_path)
        .create_disk_image(&uefi_img)
        .expect("Failed to create UEFI disk image");
    println!("UEFI image created: {}", uefi_img.display());

    // Create BIOS disk image
    let bios_img = output_path("smartos-bios.img");
    bootloader::BiosBoot::new(&kernel_path)
        .create_disk_image(&bios_img)
        .expect("Failed to create BIOS disk image");
    println!("BIOS image created: {}", bios_img.display());

    println!();
    println!("To boot with QEMU (UEFI):");
    println!(
        "  qemu-system-x86_64 -bios OVMF.fd -drive format=raw,file={} -serial stdio",
        uefi_img.display()
    );
    println!();
    println!("To boot with QEMU (BIOS):");
    println!(
        "  qemu-system-x86_64 -drive format=raw,file={} -serial stdio",
        bios_img.display()
    );
}

fn find_kernel_binary() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();

    let candidates = [
        workspace_root.join("target/x86_64-unknown-none/release/smartos-kernel"),
        workspace_root.join("target/x86_64-unknown-none/debug/smartos-kernel"),
    ];

    for path in &candidates {
        if path.exists() {
            return path.clone();
        }
    }

    eprintln!("Error: Kernel binary not found. Build it first:");
    eprintln!("  cargo build -p smartos-kernel --target x86_64-unknown-none -Zbuild-std=core,compiler_builtins,alloc -Zbuild-std-features=compiler-builtins-mem");
    eprintln!();
    eprintln!("Searched in:");
    for path in &candidates {
        eprintln!("  {}", path.display());
    }
    std::process::exit(1);
}

fn output_path(name: &str) -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let out_dir = workspace_root.join("target");
    std::fs::create_dir_all(&out_dir).ok();
    out_dir.join(name)
}
