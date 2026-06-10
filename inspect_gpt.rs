use std::path::Path;
fn main() {
    let disk = gpt::GptConfig::new().open("target/smartos.img").unwrap();
    println!("Partitions for target/smartos.img:");
    for (i, p) in disk.partitions().iter() {
        println!("{}: Name={:?}, Start LBA={}, End LBA={}, Type={:?}", i, p.name, p.first_lba, p.last_lba, p.part_type_guid);
    }
}