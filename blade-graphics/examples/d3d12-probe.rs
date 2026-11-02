//! Probe Direct3D 12 initialization: enumerate adapters, create a device and a
//! direct command queue. Windows-only, requires the `d3d12` feature.
//!
//! Run on Windows:
//! `cargo run -p blade-graphics --features d3d12 --example d3d12-probe`

fn main() {
    #[cfg(windows)]
    {
        match blade_graphics::d3d12::init() {
            Ok(init) => {
                println!("adapter: {}", init.adapter_info.name);
                println!(
                    "vendor: 0x{:04X}, device: 0x{:04X}",
                    init.adapter_info.vendor_id, init.adapter_info.device_id
                );
                println!(
                    "dedicated video memory: {} MiB",
                    init.adapter_info.dedicated_video_memory / (1024 * 1024)
                );
                println!("software adapter: {}", init.adapter_info.is_software);
                println!("nodes: {}", init.node_count);
                println!("D3D12 initialization: OK");
            }
            Err(err) => {
                eprintln!("D3D12 initialization failed: {err}");
                std::process::exit(1);
            }
        }
    }

    #[cfg(not(windows))]
    println!("d3d12-probe is Windows-only");
}
