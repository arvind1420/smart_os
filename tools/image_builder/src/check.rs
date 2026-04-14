fn main() {
    let kernel = "target/x86_64-unknown-none/release/smartos-kernel";
    let out = "target/base_raw.img";
    bootloader::UefiBoot::new(std::path::Path::new(kernel))
        .create_disk_image(std::path::Path::new(out))
        .expect("Failed");
    let mut f = std::fs::File::open(out).unwrap();
    let mut buf = [0u8; 3];
    use std::io::Read;
    f.read_exact(&mut buf).unwrap();
    println!("HEADER: {:02X} {:02X} {:02X}", buf[0], buf[1], buf[2]);
}
