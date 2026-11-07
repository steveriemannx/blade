//! Textured quad on the D3D12 device layer: upload a procedural texture, bind
//! it through an SRV descriptor table with a static sampler in the root
//! signature, draw a fullscreen triangle and verify the sampled pixels.
//!
//! Windows-only, requires the `d3d12` feature and DXC (`BLADE_DXC_PATH`). Runs on
//! the hardware adapter and on WARP.
//!
//! Run on Windows:
//! `cargo run -p blade-graphics --features d3d12 --example d3d12-texture`

#[cfg(windows)]
fn textured_quad(
    options: &blade_graphics::d3d12::InitOptions,
) -> Result<(), blade_graphics::d3d12::Error> {
    use blade_graphics::d3d12::{
        DXGI_FORMAT_R8G8B8A8_UNORM, DescriptorHeap, DescriptorHeapType, DescriptorRangeKind, Error,
        RenderPipelineDesc, ResourceState, RootDescriptorTable, RootParameter, ShaderCompiler,
        StaticSampler, TextureDesc, shader,
    };

    let init = blade_graphics::d3d12::init_with_options(options)?;
    let device = &init.device;
    eprintln!(
        "device: {} (software: {})",
        init.adapter_info.name, init.adapter_info.is_software
    );

    let compiler = ShaderCompiler::load()?;
    let vs = compiler.compile_hlsl(shader::TEXTURED_HLSL, "vs_main", "vs_6_0")?;
    let ps = compiler.compile_hlsl(shader::TEXTURED_HLSL, "ps_main", "ps_6_0")?;
    let root = device.create_graphics_root_signature_with_samplers(
        &[RootParameter::DescriptorTable(RootDescriptorTable {
            kind: DescriptorRangeKind::Srv,
            count: 1,
            base_shader_register: 0,
        })],
        &[StaticSampler::default()],
    )?;
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
    eprintln!("pipeline: SRV table t0 + static linear/clamp sampler s0");

    // 32x32 texture with a diagonal gradient plus a checker shade.
    let size = 32u32;
    let mut texels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let checker = if (x / 4 + y / 4) % 2 == 0 { 0 } else { 64 };
            texels.extend_from_slice(&[(x * 8) as u8, (y * 8) as u8, (128 + checker) as u8, 255]);
        }
    }
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

    let mut encoder = device.record()?;
    encoder.transition_texture(&texture, ResourceState::ShaderRead);
    encoder.transition_texture(&target, ResourceState::RenderTarget);
    encoder.set_render_targets(&rtv_heap, &[rtv]);
    encoder.clear_render_target(&rtv_heap, rtv, [0.0, 0.0, 0.0, 1.0]);
    encoder.set_render_pipeline(&pipeline);
    encoder.set_descriptor_heap(&srv_heap);
    encoder.set_graphics_descriptor_table(0, &srv_heap, srv);
    encoder.set_viewport(size as f32, size as f32);
    encoder.set_scissor(size as i32, size as i32);
    encoder.draw_instanced(3, 1);
    encoder.transition_texture(&target, ResourceState::CopySource);
    let sp = device.submit(encoder)?;
    device.wait(sp)?;
    device.check_device()?;

    let sampled = device.read_texture(&target)?;
    let mut worst = 0i32;
    let mut mismatched = 0usize;
    for (sampled_texel, source_texel) in sampled.chunks_exact(4).zip(texels.chunks_exact(4)) {
        let mut texel_wrong = false;
        for channel in 0..3 {
            let delta = i32::from(sampled_texel[channel]) - i32::from(source_texel[channel]);
            worst = worst.max(delta.abs());
            if delta.abs() > 2 {
                texel_wrong = true;
            }
        }
        if texel_wrong {
            mismatched += 1;
        }
    }
    if mismatched != 0 {
        eprintln!("{mismatched} texels sampled incorrectly (worst delta {worst})");
        return Err(Error::InvalidState(
            "texture sampling produced wrong colors",
        ));
    }
    eprintln!(
        "sampled {size}x{size} texture through SRV + static sampler: all texels match (worst delta {worst})"
    );
    Ok(())
}

#[cfg(windows)]
fn storage_texture_pass(
    options: &blade_graphics::d3d12::InitOptions,
) -> Result<(), blade_graphics::d3d12::Error> {
    use blade_graphics::d3d12::{
        DXGI_FORMAT_R8G8B8A8_UNORM, DescriptorHeap, DescriptorHeapType, DescriptorRangeKind, Error,
        ResourceState, RootDescriptorTable, ShaderCompiler, TextureDesc, shader,
    };

    let init = blade_graphics::d3d12::init_with_options(options)?;
    let device = &init.device;
    let compiler = ShaderCompiler::load()?;
    let dxil = compiler.compile_hlsl(shader::COMPUTE_TEXTURE_WRITE_HLSL, "main", "cs_6_0")?;
    let root = device.create_compute_root_signature(&[RootDescriptorTable {
        kind: DescriptorRangeKind::Uav,
        count: 1,
        base_shader_register: 0,
    }])?;
    let pipeline = device.create_compute_pipeline(&dxil, &root)?;

    let size = 8u32;
    let texture = device.create_texture(
        TextureDesc::new(size, size, DXGI_FORMAT_R8G8B8A8_UNORM).with_unordered_access(),
    )?;
    let copied_texture =
        device.create_texture(TextureDesc::new(size, size, DXGI_FORMAT_R8G8B8A8_UNORM))?;
    let heap = DescriptorHeap::new(device, DescriptorHeapType::CbvSrvUav, 4)?;
    let uav = device.create_texture_uav(&heap, &texture)?;

    let mut encoder = device.record()?;
    encoder.transition_texture(&texture, ResourceState::UnorderedAccess);
    encoder.set_descriptor_heap(&heap);
    encoder.set_compute_pipeline(&pipeline);
    encoder.set_compute_descriptor_table(0, &heap, uav);
    encoder.dispatch([2, 2, 1]);
    encoder.uav_barrier_texture(&texture);
    encoder.transition_texture(&texture, ResourceState::CopySource);
    encoder.transition_texture(&copied_texture, ResourceState::CopyDest);
    encoder.copy_texture(&copied_texture, &texture);
    encoder.transition_texture(&copied_texture, ResourceState::CopySource);
    let sp = device.submit(encoder)?;
    device.wait(sp)?;
    device.check_device()?;

    let written = device.read_texture(&texture)?;
    let copied = device.read_texture(&copied_texture)?;
    for (index, pixel) in written.chunks_exact(4).enumerate() {
        let x = (index as u32) % size;
        let y = (index as u32) / size;
        let expected = [
            (((x as f32 + 0.5) / 4.0) * 255.0).round() as u8,
            (((y as f32 + 0.5) / 4.0) * 255.0).round() as u8,
            128,
            255,
        ];
        for channel in 0..4 {
            let delta = i32::from(pixel[channel]) - i32::from(expected[channel]);
            if delta.abs() > 1 {
                eprintln!(
                    "texel {index} channel {channel}: {} != {}",
                    pixel[channel], expected[channel]
                );
                return Err(Error::InvalidState(
                    "storage texture write produced wrong texels",
                ));
            }
        }
    }
    let wrong = copied
        .chunks_exact(4)
        .zip(written.chunks_exact(4))
        .filter(|(copied, written)| copied != written)
        .count();
    if wrong != 0 {
        eprintln!("{wrong} texels differ after the texture copy");
        return Err(Error::InvalidState("texture copy produced wrong texels"));
    }
    eprintln!(
        "storage texture: compute wrote {size}x{size} via UAV, copied to a second texture, all texels verified"
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
            if let Err(err) = textured_quad(&options) {
                eprintln!("{label} textured quad failed: {err}");
                failed = true;
            }
            if let Err(err) = storage_texture_pass(&options) {
                eprintln!("{label} storage texture pass failed: {err}");
                failed = true;
            }
        }
        if failed {
            std::process::exit(1);
        }
        eprintln!("\nD3D12 texture sampling: OK");
    }

    #[cfg(not(windows))]
    println!("d3d12-texture is Windows-only");
}
