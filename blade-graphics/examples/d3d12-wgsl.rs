//! WGSL shaders on the D3D12 device layer: naga translates WGSL to HLSL and DXC
//! compiles it to DXIL, then a fullscreen triangle is rendered and verified.
//!
//! Windows-only, requires the `d3d12` feature and DXC (`BLADE_DXC_PATH`). Runs on
//! the hardware adapter and on WARP.
//!
//! Run on Windows:
//! `cargo run -p blade-graphics --features d3d12 --example d3d12-wgsl`

#[cfg(windows)]
fn wgsl_triangle(
    options: &blade_graphics::d3d12::InitOptions,
) -> Result<(), blade_graphics::d3d12::Error> {
    use blade_graphics::d3d12::{
        DXGI_FORMAT_R8G8B8A8_UNORM, DescriptorHeap, DescriptorHeapType, Error, RenderPipelineDesc,
        ResourceState, ShaderCompiler, TextureDesc, shader,
    };

    let init = blade_graphics::d3d12::init_with_options(options)?;
    let device = &init.device;
    eprintln!(
        "device: {} (software: {})",
        init.adapter_info.name, init.adapter_info.is_software
    );

    let compiler = ShaderCompiler::load()?;
    let hlsl = compiler.translate_wgsl(shader::WGSL_FULLSCREEN, "vs_main", "vertex")?;
    eprintln!(
        "WGSL -> HLSL: {} lines, {} bytes",
        hlsl.lines().count(),
        hlsl.len()
    );
    let vs = compiler.compile_wgsl(shader::WGSL_FULLSCREEN, "vs_main", "vertex")?;
    let ps = compiler.compile_wgsl(shader::WGSL_FULLSCREEN, "ps_main", "fragment")?;
    eprintln!(
        "WGSL -> HLSL -> DXIL: vs {} bytes, ps {} bytes",
        vs.len(),
        ps.len()
    );

    let root = device.create_graphics_root_signature(&[])?;
    let pipeline = device.create_render_pipeline(
        &RenderPipelineDesc {
            vertex_dxil: &vs,
            pixel_dxil: &ps,
            color_formats: &[DXGI_FORMAT_R8G8B8A8_UNORM],
            vertex_attributes: &[],
            depth_format: None,
        },
        &root,
    )?;

    let (width, height) = (64u32, 64u32);
    let texture = device.create_texture(
        TextureDesc::new(width, height, DXGI_FORMAT_R8G8B8A8_UNORM).with_render_target(),
    )?;
    let rtv_heap = DescriptorHeap::new(device, DescriptorHeapType::Rtv, 4)?;
    let rtv = device.create_render_target_view(&rtv_heap, &texture)?;

    let mut encoder = device.record()?;
    encoder.transition_texture(&texture, ResourceState::RenderTarget);
    encoder.set_render_targets(&rtv_heap, &[rtv]);
    encoder.clear_render_target(&rtv_heap, rtv, [0.0, 0.0, 1.0, 1.0]);
    encoder.set_render_pipeline(&pipeline);
    encoder.set_viewport(width as f32, height as f32);
    encoder.set_scissor(width as i32, height as i32);
    encoder.draw_instanced(3, 1);
    encoder.transition_texture(&texture, ResourceState::CopySource);
    let sp = device.submit(encoder)?;
    device.wait(sp)?;
    device.check_device()?;

    let pixels = device.read_texture(&texture)?;
    let wrong = pixels
        .chunks_exact(4)
        .filter(|pixel| *pixel != [0u8, 255, 0, 255])
        .count();
    if wrong != 0 {
        eprintln!("{wrong} pixels are not the WGSL green");
        return Err(Error::InvalidState("WGSL pipeline produced wrong pixels"));
    }
    eprintln!(
        "rendered {width}x{height} from WGSL: all {} pixels green",
        width * height
    );
    Ok(())
}

#[cfg(windows)]
fn wgsl_textured_quad(
    options: &blade_graphics::d3d12::InitOptions,
) -> Result<(), blade_graphics::d3d12::Error> {
    use blade_graphics::d3d12::{
        DXGI_FORMAT_R8G8B8A8_UNORM, DescriptorHeap, DescriptorHeapType, DescriptorRangeKind, Error,
        RenderPipelineDesc, ResourceState, RootDescriptorTable, RootParameter, ShaderCompiler,
        TextureDesc, shader,
    };

    let init = blade_graphics::d3d12::init_with_options(options)?;
    let device = &init.device;
    let compiler = ShaderCompiler::load()?;
    let vs = compiler.compile_wgsl(shader::WGSL_TEXTURED, "vs_main", "vertex")?;
    let ps = compiler.compile_wgsl(shader::WGSL_TEXTURED, "ps_main", "fragment")?;

    // WGSL bindings: uniform -> b0 (root constants), texture -> t1 (SRV table).
    let root = device.create_graphics_root_signature(&[
        RootParameter::Constants {
            shader_register: 0,
            count: 4,
        },
        RootParameter::DescriptorTable(RootDescriptorTable {
            kind: DescriptorRangeKind::Srv,
            count: 1,
            base_shader_register: 1,
        }),
    ])?;
    let pipeline = device.create_render_pipeline(
        &RenderPipelineDesc {
            vertex_dxil: &vs,
            pixel_dxil: &ps,
            color_formats: &[DXGI_FORMAT_R8G8B8A8_UNORM],
            vertex_attributes: &[],
            depth_format: None,
        },
        &root,
    )?;

    let size = 16u32;
    let texels: Vec<u8> = (0..size * size)
        .flat_map(|index| {
            let x = index % size;
            let y = index / size;
            [(x * 16) as u8, (y * 16) as u8, 128, 255]
        })
        .collect();
    let texture =
        device.create_texture(TextureDesc::new(size, size, DXGI_FORMAT_R8G8B8A8_UNORM))?;
    device.upload_texture(&texture, &texels)?;
    let target = device.create_texture(
        TextureDesc::new(size, size, DXGI_FORMAT_R8G8B8A8_UNORM).with_render_target(),
    )?;
    let rtv_heap = DescriptorHeap::new(device, DescriptorHeapType::Rtv, 4)?;
    let srv_heap = DescriptorHeap::new(device, DescriptorHeapType::CbvSrvUav, 4)?;
    let rtv = device.create_render_target_view(&rtv_heap, &target)?;
    let srv = device.create_texture_srv(&srv_heap, &texture)?;

    let one = 1.0f32.to_bits();
    let mut encoder = device.record()?;
    encoder.transition_texture(&texture, ResourceState::ShaderRead);
    encoder.transition_texture(&target, ResourceState::RenderTarget);
    encoder.set_render_targets(&rtv_heap, &[rtv]);
    encoder.clear_render_target(&rtv_heap, rtv, [0.0, 0.0, 0.0, 1.0]);
    encoder.set_render_pipeline(&pipeline);
    encoder.set_descriptor_heap(&srv_heap);
    encoder.set_graphics_descriptor_table(1, &srv_heap, srv);
    encoder.set_graphics_root_constants(0, &[one, one, one, one]);
    encoder.set_viewport(size as f32, size as f32);
    encoder.set_scissor(size as i32, size as i32);
    encoder.draw_instanced(3, 1);
    encoder.transition_texture(&target, ResourceState::CopySource);
    let sp = device.submit(encoder)?;
    device.wait(sp)?;
    device.check_device()?;

    let pixels = device.read_texture(&target)?;
    let mismatched = pixels
        .chunks_exact(4)
        .zip(texels.chunks_exact(4))
        .filter(|(sampled, source)| {
            (0..3)
                .any(|channel| (i32::from(sampled[channel]) - i32::from(source[channel])).abs() > 2)
        })
        .count();
    if mismatched != 0 {
        eprintln!("{mismatched} texels sampled incorrectly");
        return Err(Error::InvalidState(
            "WGSL textured quad produced wrong pixels",
        ));
    }
    eprintln!("WGSL uniform + texture bindings (b0/t1): {size}x{size} sampled correctly");
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
            if let Err(err) = wgsl_triangle(&options) {
                eprintln!("{label} WGSL demo failed: {err}");
                failed = true;
            }
            if let Err(err) = wgsl_textured_quad(&options) {
                eprintln!("{label} WGSL textured demo failed: {err}");
                failed = true;
            }
        }
        if failed {
            std::process::exit(1);
        }
        eprintln!("\nD3D12 WGSL shaders: OK");
    }

    #[cfg(not(windows))]
    println!("d3d12-wgsl is Windows-only");
}
