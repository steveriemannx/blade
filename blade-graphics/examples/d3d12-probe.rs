//! Probe Direct3D 12 initialization: enumerate adapters, create a hardware
//! device and a WARP device, each with a direct command queue.
//! Windows-only, requires the `d3d12` feature.
//!
//! Run on Windows:
//! `cargo run -p blade-graphics --features d3d12 --example d3d12-probe`

fn main() {
    #[cfg(windows)]
    {
        use blade_graphics::d3d12;

        println!("== adapters ==");
        match d3d12::enumerate_adapters() {
            Ok(adapters) => {
                for (index, adapter) in adapters.iter().enumerate() {
                    println!("[{index}] {adapter}");
                }
            }
            Err(err) => println!("adapter enumeration failed: {err}"),
        }

        println!("\n== hardware device ==");
        match d3d12::init() {
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
                println!("nodes: {}", init.device.node_count());
                println!("ray tracing tier: {}", init.device.raytracing_tier());
                println!("debug layer: {}", init.device.has_debug_layer());
                println!("D3D12 initialization: OK");
            }
            Err(err) => {
                eprintln!("D3D12 initialization failed: {err}");
                std::process::exit(1);
            }
        }

        println!("\n== WARP device ==");
        match d3d12::init_with_options(&d3d12::InitOptions::headless()) {
            Ok(init) => {
                println!("adapter: {}", init.adapter_info.name);
                println!("software adapter: {}", init.adapter_info.is_software);
                println!("ray tracing tier: {}", init.device.raytracing_tier());
                println!("WARP initialization: OK");
            }
            Err(err) => {
                eprintln!("WARP initialization failed: {err}");
                std::process::exit(1);
            }
        }
    }

    #[cfg(not(windows))]
    println!("d3d12-probe is Windows-only");
}
