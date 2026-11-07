//! Visible window demo: a flip-model swapchain renders a procedurally generated
//! pattern into a real window and presents it for a few seconds, so the result
//! can be seen (and screenshotted) on an interactive desktop.
//!
//! Windows-only, requires the `d3d12` feature and DXC (`BLADE_DXC_PATH`).
//!
//! Run on Windows (interactive desktop session):
//! `cargo run -p blade-graphics --features d3d12 --example d3d12-window`

#[cfg(windows)]
fn window_demo(
    options: &blade_graphics::d3d12::InitOptions,
) -> Result<(), blade_graphics::d3d12::Error> {
    use blade_graphics::d3d12::{
        DXGI_FORMAT_R8G8B8A8_UNORM, DescriptorHeap, DescriptorHeapType, DescriptorRangeKind,
        RenderPipelineDesc, ResourceState, RootDescriptorTable, RootParameter, ShaderCompiler,
        StaticSampler, TextureDesc, shader, window,
    };

    let init = blade_graphics::d3d12::init_with_options(options)?;
    let device = &init.device;
    eprintln!(
        "device: {} (software: {})",
        init.adapter_info.name, init.adapter_info.is_software
    );

    let (width, height) = (800u32, 600u32);
    let window = window::HiddenWindow::new("Blade D3D12 demo", width, height)?;
    window.show();
    let mut swapchain = device.create_swapchain(
        &init.factory,
        window.hwnd(),
        (width, height),
        DXGI_FORMAT_R8G8B8A8_UNORM,
    )?;
    eprintln!("window: {}x{}, visible", width, height);

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

    // Procedural texture: rainbow gradient + checkerboard, so the window is
    // obviously showing rendered content.
    let size = 256u32;
    let mut texels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let checker = if (x / 32 + y / 32) % 2 == 0 { 40 } else { 0 };
            texels.extend_from_slice(&[
                (x * 255 / (size - 1)) as u8,
                (y * 255 / (size - 1)) as u8,
                (checker + (x + y) % 64) as u8,
                255,
            ]);
        }
    }
    let texture =
        device.create_texture(TextureDesc::new(size, size, DXGI_FORMAT_R8G8B8A8_UNORM))?;
    device.upload_texture(&texture, &texels)?;

    let srv_heap = DescriptorHeap::new(device, DescriptorHeapType::CbvSrvUav, 4)?;
    let srv = device.create_texture_srv(&srv_heap, &texture)?;

    let mut frame = 0u32;
    let mut presented = 0u32;
    while presented < 600 {
        let acquired = swapchain.acquire()?;
        let rendered = swapchain.texture(acquired);
        let rtv = swapchain.render_target(acquired);

        let mut encoder = device.record()?;
        encoder.transition_texture(rendered, ResourceState::RenderTarget);
        encoder.set_render_targets(swapchain.rtv_heap(), &[rtv]);
        encoder.clear_render_target(swapchain.rtv_heap(), rtv, [0.05, 0.05, 0.1, 1.0]);
        encoder.set_render_pipeline(&pipeline);
        encoder.set_descriptor_heap(&srv_heap);
        encoder.set_graphics_descriptor_table(0, &srv_heap, srv);
        encoder.set_viewport(width as f32, height as f32);
        encoder.set_scissor(width as i32, height as i32);
        encoder.draw_instanced(3, 1);
        encoder.transition_texture(rendered, ResourceState::Present);
        let sp = device.submit(encoder)?;
        device.wait(sp)?;
        swapchain.present()?;
        frame += 1;
        presented += 1;
        if frame % 120 == 0 {
            eprintln!("presented {presented} frames");
        }
    }
    device.check_device()?;
    eprintln!("demo finished; window closes now");
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
            if let Err(err) = window_demo(&options) {
                eprintln!("{label} window demo failed: {err}");
                failed = true;
            }
        }
        if failed {
            std::process::exit(1);
        }
        eprintln!("\nD3D12 window demo: OK");
    }

    #[cfg(not(windows))]
    println!("d3d12-window is Windows-only");
}
