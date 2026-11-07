//! End-to-end compute dispatch on the D3D12 device layer: compile HLSL to
//! DXIL with DXC, build a root signature and a compute pipeline, dispatch 64
//! threads and verify the UAV buffer contents after a fence wait.
//!
//! Needs `dxcompiler.dll` + `dxil.dll` (DXC release) next to the executable, on
//! `PATH`, or at `BLADE_DXC_PATH`. Runs on the hardware adapter and on WARP.
//! Windows-only, requires the `d3d12` feature.
//!
//! Run on Windows:
//! `cargo run -p blade-graphics --features d3d12 --example d3d12-compute`

#[cfg(windows)]
fn compute_dispatch(
    options: &blade_graphics::d3d12::InitOptions,
) -> Result<(), blade_graphics::d3d12::Error> {
    use blade_graphics::d3d12::{
        BufferDesc, DescriptorHeap, DescriptorHeapType, DescriptorRangeKind, Error, Memory,
        ResourceState, RootDescriptorTable, ShaderCompiler, shader,
    };

    let init = blade_graphics::d3d12::init_with_options(options)?;
    let device = &init.device;
    println!(
        "device: {} (software: {})",
        init.adapter_info.name, init.adapter_info.is_software
    );

    let compiler = ShaderCompiler::load()?;
    let dxil = compiler.compile_hlsl(shader::TRIVIAL_COMPUTE_HLSL, "main", "cs_6_0")?;
    println!("shader: cs_6_0 compiled to {} bytes", dxil.len());

    let root = device.create_compute_root_signature(&[RootDescriptorTable {
        kind: DescriptorRangeKind::Uav,
        count: 1,
        base_shader_register: 0,
    }])?;
    let pipeline = device.create_compute_pipeline(&dxil, &root)?;

    let count = 64u32;
    let byte_size = count as u64 * 4;
    let gpu =
        device.create_buffer(BufferDesc::new(byte_size, Memory::Device).with_unordered_access())?;
    let readback = device.create_buffer(BufferDesc::new(byte_size, Memory::Readback))?;
    let heap = DescriptorHeap::new(device, DescriptorHeapType::CbvSrvUav, 4)?;
    let uav = device.create_buffer_uav_structured(&heap, &gpu, 4)?;

    let mut encoder = device.record()?;
    encoder.transition_buffer(&gpu, ResourceState::UnorderedAccess);
    encoder.set_descriptor_heap(&heap);
    encoder.set_compute_pipeline(&pipeline);
    encoder.set_compute_descriptor_table(0, &heap, uav);
    encoder.dispatch([1, 1, 1]);
    encoder.uav_barrier(&gpu);
    encoder.transition_buffer(&gpu, ResourceState::CopySource);
    encoder.copy_buffer(&readback, 0, &gpu, 0, byte_size);
    let sp = device.submit(encoder)?;
    device.wait(sp)?;
    device.check_device()?;

    let bytes = readback.read()?;
    let values: Vec<u32> = bytes[..byte_size as usize]
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    let expected: Vec<u32> = (0..count).collect();
    if values != expected {
        eprintln!("dispatch output mismatch: {values:?}");
        return Err(Error::InvalidState("compute dispatch wrote wrong values"));
    }
    println!("dispatched 64 threads, verified UAV values 0..{}", count);
    device.wait_idle()?;
    Ok(())
}

fn main() {
    #[cfg(windows)]
    {
        use blade_graphics::d3d12;

        let mut failed = false;

        println!("== hardware ==");
        if let Err(err) = compute_dispatch(&d3d12::InitOptions::default()) {
            eprintln!("hardware compute failed: {err}");
            failed = true;
        }

        println!("\n== WARP ==");
        if let Err(err) = compute_dispatch(&d3d12::InitOptions::headless()) {
            eprintln!("WARP compute failed: {err}");
            failed = true;
        }

        if failed {
            std::process::exit(1);
        }
        println!("\nD3D12 compute pipeline: OK");
    }

    #[cfg(not(windows))]
    println!("d3d12-compute is Windows-only");
}
