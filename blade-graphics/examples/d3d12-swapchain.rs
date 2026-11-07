//! Swapchain + present on the D3D12 device layer: create a hidden window, a
//! flip-model swapchain, render a clear color into the back buffer, read it
//! back, present, then resize and present again.
//!
//! Windows-only, requires the `d3d12` feature. Runs on the hardware adapter and
//! on WARP (presenting to a hidden window may report `DXGI_STATUS_OCCLUDED`,
//! which is handled).
//!
//! Run on Windows:
//! `cargo run -p blade-graphics --features d3d12 --example d3d12-swapchain`

#[cfg(windows)]
fn swapchain_frames(
    options: &blade_graphics::d3d12::InitOptions,
) -> Result<(), blade_graphics::d3d12::Error> {
    use blade_graphics::d3d12::{DXGI_FORMAT_R8G8B8A8_UNORM, PresentStatus, ResourceState, window};

    let init = blade_graphics::d3d12::init_with_options(options)?;
    let device = &init.device;
    println!(
        "device: {} (software: {})",
        init.adapter_info.name, init.adapter_info.is_software
    );

    let window = window::HiddenWindow::new("blade-d3d12-swapchain", 256, 128)?;
    let mut swapchain = device.create_swapchain(
        &init.factory,
        window.hwnd(),
        (256, 128),
        DXGI_FORMAT_R8G8B8A8_UNORM,
    )?;
    eprintln!(
        "swapchain: {:?}, {} back buffers",
        swapchain.size(),
        swapchain.rtv_heap().allocated()
    );
    // DXGI cannot resize while the presentation engine still holds frames; a
    // hidden window never consumes them, so show the window.
    window.show();

    // Three frames: clear to a different color each time, read the first one
    // back, present all of them.
    for frame_index in 0..3u32 {
        let frame = swapchain.acquire()?;
        let texture = swapchain.texture(frame);
        let rtv = swapchain.render_target(frame);
        let red = (frame_index % 2) as f32;
        let color = [red, 0.25, 1.0 - red, 1.0];

        let mut encoder = device.record()?;
        encoder.transition_texture(texture, ResourceState::RenderTarget);
        encoder.clear_render_target(swapchain.rtv_heap(), rtv, color);
        if frame_index == 0 {
            encoder.transition_texture(texture, ResourceState::CopySource);
            let sp = device.submit(encoder)?;
            device.wait(sp)?;
            let pixels = device.read_texture(texture)?;
            let expected: [u8; 4] = [
                (color[0] * 255.0) as u8,
                (color[1] * 255.0) as u8,
                (color[2] * 255.0) as u8,
                255,
            ];
            println!(
                "frame 0 back buffer first pixel: {:?} (expected {expected:?})",
                &pixels[..4]
            );

            let mut encoder = device.record()?;
            encoder.transition_texture(texture, ResourceState::Present);
            let sp = device.submit(encoder)?;
            device.wait(sp)?;
        } else {
            encoder.transition_texture(texture, ResourceState::Present);
            let sp = device.submit(encoder)?;
            device.wait(sp)?;
        }

        let status = swapchain.present()?;
        let label = match status {
            PresentStatus::Presented => "presented",
            PresentStatus::Occluded => "occluded",
        };
        eprintln!("frame {frame_index}: {label}");
        device.check_device()?;
    }

    // Resize, then render and present once more.
    swapchain.resize(device, (128, 64))?;
    eprintln!("resized to {:?}", swapchain.size());
    let frame = swapchain.acquire()?;
    let texture = swapchain.texture(frame);
    let rtv = swapchain.render_target(frame);
    let mut encoder = device.record()?;
    encoder.transition_texture(texture, ResourceState::RenderTarget);
    encoder.clear_render_target(swapchain.rtv_heap(), rtv, [0.0, 1.0, 0.0, 1.0]);
    encoder.transition_texture(texture, ResourceState::Present);
    let sp = device.submit(encoder)?;
    device.wait(sp)?;
    let status = swapchain.present()?;
    device.check_device()?;
    eprintln!("after resize: {status:?}");
    Ok(())
}

fn main() {
    #[cfg(windows)]
    {
        use blade_graphics::d3d12;

        // DXGI cannot host two independent swapchain/device stacks in one
        // process on every machine, so run each adapter in a child process.
        if let Ok(mode) = std::env::var("BLADE_SWAPCHAIN_MODE") {
            let options = if mode == "warp" {
                d3d12::InitOptions::headless()
            } else {
                d3d12::InitOptions::default()
            };
            if let Err(err) = swapchain_frames(&options) {
                eprintln!("{mode} swapchain failed: {err}");
                std::process::exit(1);
            }
            return;
        }

        let exe = std::env::current_exe().expect("current exe");
        let mut failed = false;
        for mode in ["hardware", "warp"] {
            eprintln!("== {mode} ==");
            match std::process::Command::new(&exe)
                .env("BLADE_SWAPCHAIN_MODE", mode)
                .status()
            {
                Ok(status) if status.success() => {}
                Ok(status) => {
                    eprintln!("{mode} child exited with {status}");
                    failed = true;
                }
                Err(err) => {
                    eprintln!("failed to spawn {mode} child: {err}");
                    failed = true;
                }
            }
        }
        if failed {
            std::process::exit(1);
        }
        eprintln!("\nD3D12 swapchain/present: OK");
    }

    #[cfg(not(windows))]
    println!("d3d12-swapchain is Windows-only");
}
