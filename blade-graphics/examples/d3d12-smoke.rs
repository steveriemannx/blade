//! Smoke test for the phase 1 D3D12 device layer: create a buffer in each
//! memory type, copy a payload through GPU-local memory with command lists and
//! a fence, and verify the bytes round-trip.
//!
//! Runs on the hardware adapter first, then on WARP so it works over SSH on a
//! headless machine. Windows-only, requires the `d3d12` feature.
//!
//! Run on Windows:
//! `cargo run -p blade-graphics --features d3d12 --example d3d12-smoke`

#[cfg(windows)]
fn copy_roundtrip(
    options: &blade_graphics::d3d12::InitOptions,
) -> Result<usize, blade_graphics::d3d12::Error> {
    use blade_graphics::d3d12::{BufferDesc, Memory, ResourceState};

    let init = blade_graphics::d3d12::init_with_options(options)?;
    let device = &init.device;
    println!(
        "device: {} (software: {}, nodes: {})",
        init.adapter_info.name,
        init.adapter_info.is_software,
        device.node_count()
    );

    let payload: Vec<u8> = (0..64 * 1024).map(|i| ((i * 7 + 3) % 251) as u8).collect();
    let size = payload.len() as u64;

    let upload = device.create_buffer(BufferDesc::new(size, Memory::Upload))?;
    upload.write(0, &payload)?;
    let gpu = device.create_buffer(BufferDesc::new(size, Memory::Device))?;
    let readback = device.create_buffer(BufferDesc::new(size, Memory::Readback))?;
    println!(
        "buffers: upload {} B, device {} B, readback {} B (alignment {})",
        upload.size(),
        gpu.size(),
        readback.size(),
        blade_graphics::d3d12::BUFFER_ALIGNMENT,
    );

    let mut encoder = device.record()?;
    encoder.transition(&gpu, ResourceState::CopyDest);
    encoder.copy_buffer(&gpu, 0, &upload, 0, size);
    encoder.transition(&gpu, ResourceState::CopySource);
    encoder.copy_buffer(&readback, 0, &gpu, 0, size);
    encoder.transition(&gpu, ResourceState::Common);
    let sp = device.submit(encoder)?;
    device.wait(sp)?;

    let result = readback.read()?;
    let mismatched = result[..payload.len()]
        .iter()
        .zip(&payload)
        .filter(|(a, b)| a != b)
        .count();
    if mismatched != 0 {
        eprintln!("round-trip mismatch: {mismatched} bytes differ");
        return Err(blade_graphics::d3d12::Error::InvalidState(
            "copy round-trip produced wrong data",
        ));
    }
    device.wait_idle()?;
    println!("round-trip of {size} bytes: OK");
    Ok(payload.len())
}

fn main() {
    #[cfg(windows)]
    {
        use blade_graphics::d3d12;

        let mut failed = false;

        println!("== hardware ==");
        match copy_roundtrip(&d3d12::InitOptions::default()) {
            Ok(_) => {}
            Err(err) => {
                eprintln!("hardware smoke test failed: {err}");
                failed = true;
            }
        }

        println!("\n== WARP ==");
        match copy_roundtrip(&d3d12::InitOptions::headless()) {
            Ok(_) => {}
            Err(err) => {
                eprintln!("WARP smoke test failed: {err}");
                failed = true;
            }
        }

        if failed {
            std::process::exit(1);
        }
        println!("\nD3D12 device layer smoke test: OK");
    }

    #[cfg(not(windows))]
    println!("d3d12-smoke is Windows-only");
}
