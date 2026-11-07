//! Ray tracing on the D3D12 device layer: build a bottom-level and a top-level
//! acceleration structure, trace a ray with a ray-query compute shader and
//! verify the hit distance.
//!
//! Needs DXR tier 1.0+; the reference machine's GTX 1650 reports tier 0 (no DXR),
//! while WARP reports 1.1, so the demo usually runs on WARP headlessly.
//!
//! Windows-only, requires the `d3d12` feature and DXC (`BLADE_DXC_PATH`).
//!
//! Run on Windows:
//! `cargo run -p blade-graphics --features d3d12 --example d3d12-raytracing`

#[cfg(windows)]
fn ray_tracing(
    options: &blade_graphics::d3d12::InitOptions,
) -> Result<(), blade_graphics::d3d12::Error> {
    use blade_graphics::d3d12::{
        AccelerationStructureKind, BlasGeometry, BufferDesc, DescriptorHeap, DescriptorHeapType,
        DescriptorRangeKind, Error, Memory, ResourceState, RootDescriptorTable, ShaderCompiler,
        TlasInstance, shader,
    };

    let init = blade_graphics::d3d12::init_with_options(options)?;
    let device = &init.device;
    let tier = device.raytracing_tier();
    eprintln!(
        "device: {} (software: {}), ray tracing tier {tier}",
        init.adapter_info.name, init.adapter_info.is_software
    );
    if tier < 10 {
        eprintln!("ray tracing is not supported on this adapter; skipping");
        return Ok(());
    }

    // One triangle in the Z=0 plane covering the origin.
    let positions: [f32; 9] = [-1.0, -1.0, 0.0, 1.0, -1.0, 0.0, 0.0, 1.0, 0.0];
    let vertex_bytes: Vec<u8> = positions.iter().flat_map(|v| v.to_le_bytes()).collect();
    let vertices =
        device.create_buffer(BufferDesc::new(vertex_bytes.len() as u64, Memory::Upload))?;
    vertices.write(0, &vertex_bytes)?;

    let blas = device.create_bottom_level_acceleration_structure(&[BlasGeometry {
        vertices: &vertices,
        vertex_count: 3,
        vertex_stride: 12,
        indices: None,
    }])?;
    let tlas = device.create_top_level_acceleration_structure(&[TlasInstance {
        transform: [
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0,
        ],
        blas: &blas,
        instance_mask: 0xFF,
    }])?;
    assert_eq!(blas.kind(), AccelerationStructureKind::BottomLevel);
    assert_eq!(tlas.kind(), AccelerationStructureKind::TopLevel);
    eprintln!(
        "acceleration structures: blas {} B, tlas {} B",
        blas.size(),
        tlas.size()
    );

    let output =
        device.create_buffer(BufferDesc::new(4, Memory::Device).with_unordered_access())?;
    let readback = device.create_buffer(BufferDesc::new(4, Memory::Readback))?;

    let compiler = ShaderCompiler::load()?;
    let dxil = compiler.compile_hlsl(shader::RAY_QUERY_HLSL, "main", "cs_6_5")?;
    eprintln!(
        "shader: ray query compute compiled to {} bytes of DXIL",
        dxil.len()
    );

    let root = device.create_compute_root_signature(&[
        RootDescriptorTable {
            kind: DescriptorRangeKind::Srv,
            count: 1,
            base_shader_register: 0,
        },
        RootDescriptorTable {
            kind: DescriptorRangeKind::Uav,
            count: 1,
            base_shader_register: 0,
        },
    ])?;
    let pipeline = device.create_compute_pipeline(&dxil, &root)?;

    let heap = DescriptorHeap::new(device, DescriptorHeapType::CbvSrvUav, 8)?;
    let tlas_srv = device.create_acceleration_structure_srv(&heap, &tlas)?;
    let output_uav = device.create_buffer_uav_structured(&heap, &output, 4)?;

    let mut encoder = device.record()?;
    encoder.transition_buffer(&output, ResourceState::UnorderedAccess);
    encoder.set_descriptor_heap(&heap);
    encoder.set_compute_pipeline(&pipeline);
    encoder.set_compute_descriptor_table(0, &heap, tlas_srv);
    encoder.set_compute_descriptor_table(1, &heap, output_uav);
    encoder.dispatch([1, 1, 1]);
    encoder.uav_barrier(&output);
    encoder.transition_buffer(&output, ResourceState::CopySource);
    encoder.copy_buffer(&readback, 0, &output, 0, 4);
    let sp = device.submit(encoder)?;
    device.wait(sp)?;
    device.check_device()?;

    let bytes = readback.read()?;
    let hit = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    eprintln!("ray from (0, 0, -2) along +Z hit the triangle at t = {hit}");
    if (hit - 2.0).abs() > 1e-4 {
        return Err(Error::InvalidState(
            "ray query did not hit the triangle at the expected distance",
        ));
    }
    Ok(())
}

fn main() {
    #[cfg(windows)]
    {
        use blade_graphics::d3d12;

        let mut failed = false;
        for (label, options) in [
            ("hardware", d3d12::InitOptions::default()),
            ("warp", d3d12::InitOptions::headless()),
        ] {
            eprintln!("== {label} ==");
            if let Err(err) = ray_tracing(&options) {
                eprintln!("{label} ray tracing failed: {err}");
                failed = true;
            }
        }
        if failed {
            std::process::exit(1);
        }
        eprintln!("\nD3D12 ray tracing: OK");
    }

    #[cfg(not(windows))]
    println!("d3d12-raytracing is Windows-only");
}
