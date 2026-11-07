//! Smoke test for the D3D12 device layer (phases 1-2): copy a payload through
//! GPU-local buffer memory, upload a texture and read it back, and create
//! buffer/texture/sampler descriptors — all with command lists and fences.
//!
//! Runs on the hardware adapter first, then on WARP so it works over SSH on a
//! headless machine. Windows-only, requires the `d3d12` feature.
//!
//! Run on Windows:
//! `cargo run -p blade-graphics --features d3d12 --example d3d12-smoke`

#[cfg(windows)]
fn device_smoke(
    options: &blade_graphics::d3d12::InitOptions,
) -> Result<(), blade_graphics::d3d12::Error> {
    use blade_graphics::d3d12::{
        BufferDesc, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R32_FLOAT, DescriptorHeap,
        DescriptorHeapType, Error, Heap, HeapType, Memory, ResourceState, SamplerDesc, TextureDesc,
    };

    let init = blade_graphics::d3d12::init_with_options(options)?;
    let device = &init.device;
    println!(
        "device: {} (software: {}, nodes: {})",
        init.adapter_info.name,
        init.adapter_info.is_software,
        device.node_count()
    );

    // 1. Buffer round-trip through device-local memory.
    let payload: Vec<u8> = (0..64 * 1024).map(|i| ((i * 7 + 3) % 251) as u8).collect();
    let size = payload.len() as u64;
    let upload = device.create_buffer(BufferDesc::new(size, Memory::Upload))?;
    upload.write(0, &payload)?;
    let gpu =
        device.create_buffer(BufferDesc::new(size, Memory::Device).with_unordered_access())?;
    let readback = device.create_buffer(BufferDesc::new(size, Memory::Readback))?;
    println!(
        "buffers: upload {} B, device {} B, readback {} B (alignment {})",
        upload.size(),
        gpu.size(),
        readback.size(),
        blade_graphics::d3d12::BUFFER_ALIGNMENT,
    );

    let mut encoder = device.record()?;
    encoder.transition_buffer(&gpu, ResourceState::CopyDest);
    encoder.copy_buffer(&gpu, 0, &upload, 0, size);
    encoder.transition_buffer(&gpu, ResourceState::CopySource);
    encoder.copy_buffer(&readback, 0, &gpu, 0, size);
    encoder.transition_buffer(&gpu, ResourceState::Common);
    let sp = device.submit(encoder)?;
    device.wait(sp)?;

    let result = readback.read()?;
    if result[..payload.len()] != payload[..] {
        eprintln!("buffer round-trip mismatch");
        return Err(Error::InvalidState("buffer round-trip produced wrong data"));
    }
    println!("buffer round-trip of {size} bytes: OK");

    // 2. Texture upload + readback.
    let (width, height) = (128u32, 64u32);
    let texture =
        device.create_texture(TextureDesc::new(width, height, DXGI_FORMAT_R8G8B8A8_UNORM))?;
    let texels: Vec<u8> = (0..width * height)
        .flat_map(|i| [(i % 251) as u8, 0x10, 0x20, 0xFF])
        .collect();
    device.upload_texture(&texture, &texels)?;
    let texels_back = device.read_texture(&texture)?;
    if texels_back != texels {
        eprintln!("texture round-trip mismatch");
        return Err(Error::InvalidState(
            "texture round-trip produced wrong data",
        ));
    }
    println!(
        "texture {width}x{height} upload + readback ({} KiB): OK",
        texels.len() / 1024
    );

    // 3. Descriptor heaps, views and a sampler.
    let view_heap = DescriptorHeap::new(device, DescriptorHeapType::CbvSrvUav, 256)?;
    let sampler_heap = DescriptorHeap::new(device, DescriptorHeapType::Sampler, 16)?;
    let _buffer_srv = device.create_buffer_srv_typed(&view_heap, &gpu, DXGI_FORMAT_R32_FLOAT)?;
    let _buffer_uav = device.create_buffer_uav_typed(&view_heap, &gpu, DXGI_FORMAT_R32_FLOAT)?;
    let _buffer_cbv = device.create_buffer_cbv(&view_heap, &gpu, 0, 256)?;
    let texture_srv = device.create_texture_srv(&view_heap, &texture)?;
    let sampler = device.create_sampler(&sampler_heap, &SamplerDesc::default())?;
    println!(
        "descriptors: view heap {}/{}, sampler heap {}/{} (texture srv at {}, sampler at {})",
        view_heap.allocated(),
        view_heap.capacity(),
        sampler_heap.allocated(),
        sampler_heap.capacity(),
        texture_srv.start,
        sampler.start,
    );

    // 4. Placed resources in explicit heaps, plus residency churn.
    let device_heap = Heap::new(device, HeapType::Default, 256 * 1024)?;
    let upload_heap = Heap::new(device, HeapType::Upload, 256 * 1024)?;
    let placed_offset = device_heap.allocate(1024).expect("heap range");
    let placed = device.create_placed_buffer(
        &device_heap,
        placed_offset,
        BufferDesc::new(1024, Memory::Device),
    )?;
    let upload_offset = upload_heap.allocate(1024).expect("upload range");
    let placed_upload = device.create_placed_buffer(
        &upload_heap,
        upload_offset,
        BufferDesc::new(1024, Memory::Upload),
    )?;
    let placed_readback = device.create_buffer(BufferDesc::new(1024, Memory::Readback))?;
    let placed_payload: Vec<u8> = (0..1024).map(|index| (index % 251) as u8).collect();
    placed_upload.write(0, &placed_payload)?;
    let mut encoder = device.record()?;
    encoder.transition_buffer(&placed, ResourceState::CopyDest);
    encoder.copy_buffer(&placed, 0, &placed_upload, 0, 1024);
    encoder.transition_buffer(&placed, ResourceState::CopySource);
    encoder.copy_buffer(&placed_readback, 0, &placed, 0, 1024);
    let sp = device.submit(encoder)?;
    device.wait(sp)?;
    let placed_bytes = placed_readback.read()?;
    if placed_bytes[..1024] != placed_payload[..] {
        return Err(Error::InvalidState(
            "placed buffer round-trip produced wrong data",
        ));
    }
    device.evict(&[&placed])?;
    device.make_resident(&[&placed])?;
    println!(
        "placed resources: heap {} KiB, {} KiB allocated, round-trip + evict/resident OK",
        device_heap.size() / 1024,
        device_heap.allocated() / 1024
    );

    // 5. Placed texture in the same heap, uploaded and read back.
    let texture_desc = TextureDesc::new(32, 32, DXGI_FORMAT_R8G8B8A8_UNORM);
    let (texture_size, _alignment) = device.texture_allocation_size(&texture_desc)?;
    let texture_offset = device_heap.allocate(texture_size).expect("texture range");
    let placed_texture =
        device.create_placed_texture(&device_heap, texture_offset, texture_desc)?;
    let texels: Vec<u8> = (0..32u32 * 32)
        .flat_map(|index| [(index % 251) as u8, 0x10, 0x20, 0xFF])
        .collect();
    device.upload_texture(&placed_texture, &texels)?;
    let readback_texels = device.read_texture(&placed_texture)?;
    if readback_texels != texels {
        return Err(Error::InvalidState(
            "placed texture round-trip produced wrong texels",
        ));
    }
    println!(
        "placed texture: {}x{} in {} KiB heap, upload + readback OK",
        placed_texture.width(),
        placed_texture.height(),
        device_heap.size() / 1024
    );

    device.wait_idle()?;
    Ok(())
}

fn main() {
    #[cfg(windows)]
    {
        use blade_graphics::d3d12;

        let mut failed = false;

        println!("== hardware ==");
        if let Err(err) = device_smoke(&d3d12::InitOptions::default()) {
            eprintln!("hardware smoke test failed: {err}");
            failed = true;
        }

        println!("\n== WARP ==");
        if let Err(err) = device_smoke(&d3d12::InitOptions::headless()) {
            eprintln!("WARP smoke test failed: {err}");
            failed = true;
        }

        if failed {
            std::process::exit(1);
        }
        println!("\nD3D12 device layer smoke test: OK");
    }

    #[cfg(not(windows))]
    println!("d3d12-smoke is Windows-only");
}
