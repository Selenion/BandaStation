use bandastation_atmos::{cell::Cell, network::Tile, Mixture};
use std::mem::size_of;

fn main() {
    println!("pointer width: {} bits", usize::BITS);
    println!("Mixture inline: {} bytes", size_of::<Mixture>());
    println!("Cell inline: {} bytes", size_of::<Cell>());
    println!("Tile inline: {} bytes", size_of::<Tile>());
    println!(
        "Two 255x255 frames, inline fields only: {:.2} MiB/z",
        (2 * 255 * 255 * (size_of::<Cell>() + size_of::<Tile>())) as f64 / (1024.0 * 1024.0)
    );
    println!("Excludes map nodes, Vec storage, planetary boxes, queued commands and allocator overhead; this is not RSS.");
}
