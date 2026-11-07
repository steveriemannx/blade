//! Vertex buffers and depth testing on the D3D12 device layer: compile a mesh
//! pipeline with an input layout, upload vertex data, draw two overlapping
//! triangles with different depths and verify that depth testing keeps the near
//! one.
//!
//! Windows-only, requires the `d3d12` feature and DXC (`BLADE_DXC_PATH`). Runs on
//! the hardware adapter and on WARP.
//!
//! Run on Windows:
//! `cargo run -p blade-graphics --features d3d12 --example d3d12-mesh`

#[cfg(windows)]
fn mesh_demo(
    options: &blade_graphics::d3d12::InitOptions,
) -> Result<(), blade_graphics::d3d12::Error> {
    use blade_graphics::d3d12::{
        BufferDesc, DXGI_FORMAT_D32_FLOAT, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R32G32B32_FLOAT,
        DescriptorHeap, DescriptorHeapType, Error, Memory, RenderPipelineDesc, ResourceState,
        ShaderCompiler, TextureDesc, VertexAttribute, shader,
    };

    let init = blade_graphics::d3d12::init_with_options(options)?;
    let device = &init.device;
    eprintln!(
        "device: {} (software: {})",
        init.adapter_info.name, init.adapter_info.is_software
    );

    let compiler = ShaderCompiler::load()?;
    let vs = compiler.compile_hlsl(shader::MESH_HLSL, "vs_main", "vs_6_0")?;
    let ps = compiler.compile_hlsl(shader::MESH_HLSL, "ps_main", "ps_6_0")?;
    let root = device.create_graphics_root_signature(&[])?;
    let pipeline = device.create_render_pipeline(
        &RenderPipelineDesc {
            vertex_dxil: &vs,
            pixel_dxil: &ps,
            color_formats: &[DXGI_FORMAT_R8G8B8A8_UNORM],
            vertex_attributes: &[
                VertexAttribute {
                    semantic: "POSITION",
                    format: DXGI_FORMAT_R32G32B32_FLOAT,
                    offset: 0,
                },
                VertexAttribute {
                    semantic: "COLOR",
                    format: DXGI_FORMAT_R32G32B32_FLOAT,
                    offset: 12,
                },
            ],
            depth_format: Some(DXGI_FORMAT_D32_FLOAT),
        },
        &root,
    )?;
    eprintln!("pipeline: input layout POSITION+COLOR, depth test LESS, vs/ps from DXC");

    let make_triangle = |z: f32, color: [f32; 3]| -> Vec<u8> {
        let positions = [(-1.0f32, 1.0f32), (3.0, 1.0), (-1.0, -3.0)];
        positions
            .iter()
            .flat_map(|(x, y)| [*x, *y, z, color[0], color[1], color[2]])
            .flat_map(|value| value.to_le_bytes())
            .collect()
    };
    let upload = |data: Vec<u8>| -> Result<_, Error> {
        let buffer = device.create_buffer(BufferDesc::new(data.len() as u64, Memory::Upload))?;
        buffer.write(0, &data)?;
        Ok(buffer)
    };
    let near_vertices = upload(make_triangle(0.25, [1.0, 0.0, 0.0]))?;
    let far_vertices = upload(make_triangle(0.75, [0.0, 1.0, 0.0]))?;
    eprintln!("vertex buffers: near (z=0.25, red), far (z=0.75, green)");

    let (width, height) = (64u32, 64u32);
    let texture = device.create_texture(
        TextureDesc::new(width, height, DXGI_FORMAT_R8G8B8A8_UNORM).with_render_target(),
    )?;
    let depth_texture = device.create_texture(
        TextureDesc::new(width, height, DXGI_FORMAT_D32_FLOAT).with_depth_stencil(),
    )?;
    let rtv_heap = DescriptorHeap::new(device, DescriptorHeapType::Rtv, 4)?;
    let dsv_heap = DescriptorHeap::new(device, DescriptorHeapType::Dsv, 4)?;
    let rtv = device.create_render_target_view(&rtv_heap, &texture)?;
    let dsv = device.create_depth_stencil_view(&dsv_heap, &depth_texture)?;

    let mut encoder = device.record()?;
    encoder.transition_texture(&texture, ResourceState::RenderTarget);
    encoder.transition_texture(&depth_texture, ResourceState::DepthWrite);
    encoder.set_render_targets_with_depth(&rtv_heap, &[rtv], &dsv_heap, dsv);
    encoder.clear_render_target(&rtv_heap, rtv, [0.0, 0.0, 1.0, 1.0]);
    encoder.clear_depth_stencil(&dsv_heap, dsv, 1.0);
    encoder.set_render_pipeline(&pipeline);
    encoder.set_viewport(width as f32, height as f32);
    encoder.set_scissor(width as i32, height as i32);
    encoder.set_vertex_buffer(&near_vertices, 24);
    encoder.draw_instanced(3, 1);
    encoder.set_vertex_buffer(&far_vertices, 24);
    encoder.draw_instanced(3, 1);
    encoder.transition_texture(&texture, ResourceState::CopySource);
    let sp = device.submit(encoder)?;
    device.wait(sp)?;
    device.check_device()?;

    let pixels = device.read_texture(&texture)?;
    let red = [255u8, 0, 0, 255];
    let wrong = pixels.chunks_exact(4).filter(|pixel| *pixel != red).count();
    if wrong != 0 {
        eprintln!("{wrong} pixels are not the near (red) triangle");
        return Err(Error::InvalidState(
            "depth test did not keep the near triangle",
        ));
    }
    eprintln!(
        "rendered {width}x{height}: all {} pixels are the near red triangle (far green was depth-rejected)",
        width * height
    );
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
            if let Err(err) = mesh_demo(&options) {
                eprintln!("{label} mesh demo failed: {err}");
                failed = true;
            }
        }
        if failed {
            std::process::exit(1);
        }
        eprintln!("\nD3D12 vertex buffers + depth test: OK");
    }

    #[cfg(not(windows))]
    println!("d3d12-mesh is Windows-only");
}
