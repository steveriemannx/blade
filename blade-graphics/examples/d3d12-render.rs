//! Offscreen render pass on the D3D12 device layer: compile vertex + pixel HLSL
//! with DXC, build a graphics root signature (root constants) and pipeline,
//! clear an RTV, draw a fullscreen triangle and verify the pixels after a fence
//! wait.
//!
//! Needs `dxcompiler.dll` + `dxil.dll` (DXC release) next to the executable, on
//! `PATH`, or at `BLADE_DXC_PATH`. Runs on the hardware adapter and on WARP.
//! Windows-only, requires the `d3d12` feature.
//!
//! Run on Windows:
//! `cargo run -p blade-graphics --features d3d12 --example d3d12-render`

#[cfg(windows)]
fn render_triangle(
    options: &blade_graphics::d3d12::InitOptions,
) -> Result<(), blade_graphics::d3d12::Error> {
    use blade_graphics::d3d12::{
        DXGI_FORMAT_R8G8B8A8_UNORM, DescriptorHeap, DescriptorHeapType, Error, RenderPipelineDesc,
        ResourceState, RootParameter, ShaderCompiler, TextureDesc, shader,
    };

    let init = blade_graphics::d3d12::init_with_options(options)?;
    let device = &init.device;
    println!(
        "device: {} (software: {})",
        init.adapter_info.name, init.adapter_info.is_software
    );

    let compiler = ShaderCompiler::load()?;
    let vs = compiler.compile_hlsl(shader::FULLSCREEN_HLSL, "vs_main", "vs_6_0")?;
    let ps = compiler.compile_hlsl(shader::FULLSCREEN_HLSL, "ps_main", "ps_6_0")?;
    println!(
        "shaders: vs_6_0 {} bytes, ps_6_0 {} bytes",
        vs.len(),
        ps.len()
    );

    let root = device.create_graphics_root_signature(&[RootParameter::Constants {
        shader_register: 0,
        count: 4,
    }])?;
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

    let (width, height) = (128u32, 64u32);
    let texture = device.create_texture(
        TextureDesc::new(width, height, DXGI_FORMAT_R8G8B8A8_UNORM).with_render_target(),
    )?;
    let rtv_heap = DescriptorHeap::new(device, DescriptorHeapType::Rtv, 4)?;
    let rtv = device.create_render_target_view(&rtv_heap, &texture)?;

    let green = [0.0f32, 1.0, 0.0, 1.0];
    let green_bits: Vec<u32> = green.iter().map(|channel| channel.to_bits()).collect();

    let mut encoder = device.record()?;
    encoder.transition_texture(&texture, ResourceState::RenderTarget);
    encoder.set_render_targets(&rtv_heap, &[rtv]);
    encoder.clear_render_target(&rtv_heap, rtv, [0.0, 0.0, 1.0, 1.0]);
    encoder.set_render_pipeline(&pipeline);
    encoder.set_viewport(width as f32, height as f32);
    encoder.set_scissor(width as i32, height as i32);
    encoder.set_graphics_root_constants(0, &green_bits);
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
        eprintln!("{wrong} pixels were not green (the clear color survived)");
        return Err(Error::InvalidState("render pass produced wrong pixels"));
    }
    println!(
        "cleared to blue, drew a fullscreen green triangle: {}x{}, all {} pixels green",
        width,
        height,
        width * height
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
        if let Err(err) = render_triangle(&d3d12::InitOptions::default()) {
            eprintln!("hardware render failed: {err}");
            failed = true;
        }

        println!("\n== WARP ==");
        if let Err(err) = render_triangle(&d3d12::InitOptions::headless()) {
            eprintln!("WARP render failed: {err}");
            failed = true;
        }

        if failed {
            std::process::exit(1);
        }
        println!("\nD3D12 render pipeline: OK");
    }

    #[cfg(not(windows))]
    println!("d3d12-render is Windows-only");
}
