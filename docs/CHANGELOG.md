Changelog for *Blade* project

## (TBD)

- build: disable winit's default `wayland-csd-adwaita` feature, dropping the Apache-2.0-only `ab_glyph`/`owned_ttf_parser` chain; Wayland windows no longer get client-side decorations
- graphics: add a Windows-only `d3d12` feature with a Direct3D 12 device-initialization probe and a backend design document (`blade-graphics/etc/d3d12.md`)
- graphics: grow the Windows-only `d3d12` backend into a device layer — adapter enumeration, debug layer, WARP fallback for headless tests, a direct queue with fence synchronization, committed buffers, resource transitions and buffer copies, covered by Windows unit tests and `d3d12-probe` / `d3d12-smoke` examples
- graphics: the Windows-only `d3d12` device layer gains 2D textures with footprint-based upload/readback, shader-visible descriptor heaps with CBV/SRV/UAV and sampler views, buffer UAV flags, and a device-removed check; the Windows smoke example now covers textures and descriptors too
- graphics: the Windows-only `d3d12` backend gets a runtime DXC shader toolchain (HLSL -> DXIL with diagnostics), serialized compute root signatures, compute pipeline states and command-list dispatch/UAV barriers; a compute example dispatches 64 threads and verifies the UAV contents on hardware and WARP
- graphics: the Windows-only `d3d12` backend adds the offscreen render path — RTV heaps and render-target textures, graphics root signatures with 32-bit root constants, graphics pipeline states, viewport/scissor/clear/draw recording; a render example draws a fullscreen triangle and verifies every pixel on hardware and WARP
- graphics: the Windows-only `d3d12` backend adds swapchain/present — `CreateSwapChainForHwnd` and composition swapchains with FLIP_DISCARD and frame-latency pacing, present-status handling, back-buffer readback and `ResizeBuffers`; DXGI/D3D12 info-queue diagnostics and a live-object report; per-submission command allocator lifetime tracking and borrow-based D3D12 barrier/copy locations fix leaked COM references that made `ResizeBuffers` fail
- graphics: the Windows-only `d3d12` backend adds vertex input layouts and depth testing — `VertexAttribute`/input-layout PSOs, DSV heaps and depth-stencil textures, `set_vertex_buffer`/`set_index_buffer`/`draw_indexed`, depth clear and `set_render_targets_with_depth`; a mesh example draws depth-tested triangles and verifies the near surface wins on hardware and WARP
- graphics: the Windows-only `d3d12` backend adds texture sampling and multiple render targets — static samplers baked into root signatures, graphics SRV descriptor tables, `color_formats`/multi-RTV binding for `SV_Target0..N`; a textured-quad example verifies every sampled texel on hardware and WARP
- graphics: the Windows-only `d3d12` backend caches root signatures and pipeline states by content hash (with hit/miss stats) and detects device loss, surfacing `Error::DeviceLost` from `record`/`submit`/`wait`/resource creation; a debug-layer `RemoveDevice` test covers the loss path
- graphics: the Windows-only `d3d12` backend wires the WGSL front end — naga `hlsl-out` translates WGSL to HLSL and DXC compiles it to DXIL, so shaders can be authored in WGSL; a new example renders a WGSL fullscreen triangle on hardware and WARP
- graphics: the Windows-only `d3d12` backend maps WGSL `@group(0)` resource bindings onto HLSL registers (uniform buffers to root constants, textures to SRV tables), so resource-bound WGSL shaders run; sampler bindings remain on the naga sampler-heap model and are not wired yet
- graphics: the Windows-only `d3d12` backend adds DXR ray tracing — bottom/top-level acceleration structure builds, acceleration-structure SRVs and inline ray-query compute shaders (`cs_6_5`), verified by tracing a ray against a triangle on WARP; `raytracing_tier` reports adapter support
- graphics: the Windows-only `d3d12` backend adds storage textures — `TextureDesc::with_unordered_access()`, 2D texture UAVs, whole-texture copies and texture UAV barriers; a compute shader writes a storage texture and a copy is verified texel by texel
- graphics: the Windows-only `d3d12` backend adds explicit heaps and residency — 64 KiB bump allocator over `ID3D12Heap`, placed buffers via `CreatePlacedResource`, and `MakeResident`/`Evict`; the smoke example round-trips a placed buffer through the GPU and exercises eviction
- graphics: the Windows-only `d3d12` backend adds placed textures — `texture_allocation_size` (resource allocation info) and `create_placed_texture`; a rendered placed render target is copied to a second placed texture and verified, and the smoke example round-trips an uploaded placed texture
- graphics: the Windows-only `d3d12` backend adds DXR 1.1 ray-tracing pipelines — DXIL library compilation (`lib_6_3`), state objects with hit groups and global root signatures, shader binding tables and `DispatchRays`; the ray-tracing example traces a ray through both ray queries and a pipeline on WARP
- graphics: the Windows-only `d3d12` backend gains a visible-window demo (`d3d12-window`) that renders a procedural pattern into a flip-model swapchain and presents it, verified by screenshot on an interactive desktop session
- render: retain SVGF history on frozen frames; vehicle diagnostic captures require `BLADE_CAPTURE_VEHICLE=1` even when running ignored tests
- graphics: ray queries require macOS 13+ / iOS 16+ on Metal; compute limits report direct sampled-texture slots, and renderer resource tables account for Vulkan image descriptors
- render: `GBufferViews::motion` is now `Rgba16Float`, with screen-space motion in XY and previous-hit distance in Z
- graphics: `ResourceArray::with_limit` allocates runtime tables below their generic index ceiling
- graphics: Vulkan explicitly marks non-indexed acceleration-structure geometry, avoiding a null index-buffer read during BLAS builds
- render: bootstrap variance for newly visible surfaces from raw lighting, with separate raw/temporal targets to avoid in-place neighborhood read races
- render: keep SVGF history when resetting debug pixel statistics; share temporal/spatial guide work across diffuse and specular lighting without changing resolution or pass count
- graphics: Metal binding arrays query populated resource handles once before padding the argument table, avoiding repeated Objective-C validation for unused slots
- render: preserve SVGF edges with shading-normal/material guides, point-to-plane depth tests, previous-hit depth for moving geometry, and temporal luminance variance; normal transforms use the inverse transpose under nonuniform scale
- render: represent rigid poses shared by a model's unskinned primitives as instance transforms rather than skinning/refitting a BLAS, and reuse identical current/previous vertex-table entries
- graphics: Metal timing takes completion from a real pass instead of an empty blit encoder, and handles empty submissions; remove redundant unsafe blocks and guard the macOS 15 math-mode API
- graphics: validate array bindings consistently and pad Vulkan acceleration-structure arrays to the actual layout count; render scene limits account for direct descriptor budgets
- graphics: Metal BLAS builds and refits apply geometry transforms, requiring macOS 13+ / iOS 16+; upload `AccelerationStructureTransform` values for backend-correct matrix packing
- render: size ray-tracing resource tables and scene allocation limits to the device's reported indirect-array capacity; graphics array bindings pad to the actual pipeline layout count
- graphics: Metal supports sampled-texture and storage-buffer binding arrays through argument tables, reported by `Capabilities::binding_array`; partially filled arrays repeat their resources to fill the declared length
- graphics: `Capabilities::compute_binding_limits` always reports `ComputeBindingLimits`, including separate storage-buffer, plain-data, combined-slot, bind-group, range, and indirect-array limits
- graphics: `Capabilities::device_memory_fence` reports cross-workgroup publication support; declare shared storage buffers `@coherent`, and synchronize workgroups explicitly; Metal requires Apple silicon with MSL 3.2
- graphics: buffer arrays use the ordinary `ResourceArray` / `ShaderBindable` path; Vulkan validates descriptor counts before writing, and binding empty resource arrays is rejected
- graphics: raise `limits::RESOURCES_IN_GROUP` from 8 to 64; device-specific limits still apply
- graphics: Metal `Buffer::size()` returns the accessible buffer length, not the padded allocation size
- graphics: Vulkan automatically preserves f16 subnormals when the device supports independent denormal controls
- graphics: add `CooperativeMatrix::workgroup`, `WorkgroupCooperativeMatrix`, `CooperativeMatrixGranularity`, and `CooperativeMatrixOperand` for flexible workgroup matrix products with f32 accumulators; Vulkan enables `VK_NV_cooperative_matrix2` when supported and checks shader dimensions, invocation counts, and shared memory against device limits
- graphics: `shader_bfloat16` requires BF16 cooperative matrices, even for storage and conversion alone; Vulkan enables both BF16 features together, and shader validation follows the advertised capability
- graphics: vk: build each shader stage from a module reduced to its entry point, so its SPIR-V declares only what that entry point uses; pipeline overrides no longer panic on unbound resources, and they set the reported workgroup size
- graphics: vk: expose `shader_bfloat16` and bf16/f32 cooperative-matrix shapes; enable BF16 and Vulkan-memory-model SPIR-V only when the device reports the corresponding optional features
- graphics: vk: `shader_float16` requires and enables 16-bit storage buffer access, which naga's `f16` SPIR-V declares; the auto-enable from cooperative matrix f16 support is gone
- graphics: `CommandEncoder::present_with_damage` tells the compositor which rectangles changed since the previous present, empty if none did: `VK_KHR_incremental_present` on Vulkan, `EGL_KHR/EXT_swap_buffers_with_damage` on GLES, ignored on Metal and WebGL
- graphics: `CooperativeMatrix` lists `[M, N, K]` shapes in `f32_shapes`, `f16_f32_shapes`, and `bf16_f32_shapes` instead of square tile sizes, and is no longer `Copy`
- graphics: `Capabilities` reports the default subgroup size and `max_compute_shared_memory_size`
- graphics: a pass-boundary GPU timestamp uses that barrier's source stage, so the sample is the completion that releases the next pass
- graphics: `CommandEncoder::last_timing` returns `Timing` with borrowed pass names; wait on its sync point first
- graphics: `Capabilities::timing` tells whether the device can timestamp passes at all
- engine: enable GPU timing only when the device supports it
- examples: enable directional shadows in `move`
- graphics: allow `quantizeToF16`, `pack2x16float` and `unpack2x16float` in shaders
- graphics: allow multisampled shading, cube array textures, and subgroup barriers in shaders
- render: harden directional shadow receivers for skinned meshes (normal + light-dir bias, stronger depth bias, defaults that keep characters lit under lavapipe)
- render: init the directional shadow map even when the effect is off, so Vulkan does not sample it from `UNDEFINED`


## blade-graphics-0.9, blade-util-0.5, blade-egui-0.9, blade-particle-0.2, blade-asset-0.2.2, blade-render-0.6, blade-helpers-0.3, blade-engine-0.2 (5 Sep 2026)

- MSRV raised to 1.92
- update to naga-30 and openxr-0.21
- graphics:
  - `Memory::DeviceTransient` for short-lived device-local buffers
  - `Memory::Download` uses host-cached mapping
  - pipeline statistics on Vulkan and Metal via `get_pipeline_statistics`
  - `CommandEncoderDesc::manual_barriers` and `CommandEncoder::barrier`
  - `Capabilities` now includes `compute` and `indirect_draw`
  - vk: grow descriptor pools, size them for binding arrays
  - vk: device-scope memory model and device-addressable external memory
  - vk: `VK_EXT_external_memory_host`
  - vk: derive global pass barriers from recorded pass kinds
  - vk: XR swapchain color space, public recommended surface config
  - gles: compressed BC texture uploads
  - gles: WebGL present via `texelFetch`, honor `ColorSpace`
  - gles: `sync_buffer` takes `BufferTarget` for WebGL2 index buffers
  - gles: assign sampler texture units, honor depth-stencil state
  - gles: skip 0×0 allocations, keep WebGL contexts thread-affine
- asset:
  - in-memory VFS, tolerate missing on-disk cache, non-blocking `get`
- render:
  - physically based materials from glTF metallic-roughness
    - cooked model format changed, clear asset caches
  - `RenderMode` including a canonical path tracer
    - `RayTracer::render` replaces `ray_trace` and `denoise`
  - G-buffer and radiance views for external post-process
  - optional tone mapping for HDR capture
  - subpixel projection jitter
  - local point/spot lights and directional shadow maps
  - skeletal animation from glTF skins
  - color space taken from `RenderConfig`
  - raster path on WebGL2
- engine:
  - animation playback via `Engine::set_animation`
  - opaque `JointHandle`
  - raster rendering on WebGL2
- egui:
  - WebGL2-compatible buffer uploads

## blade-egui-0.8.1, blade-util-0.4.1 (25 Apr 2026)

- fix leaking textures and non-reusable buffers in egui workflow

## blade-graphics-0.8.4 (17 Apr 2026)

- vk: use driver API version for instance creation
- vk: drop unnecessary `UNIFORM_READ` from intra-pass compute barrier
- vk: log leaked GPU memory blocks by name on context teardown

## blade-graphics-0.8.3 (17 Apr 2026)

- vk: fix descriptor over-allocation for uniform data

## blade-graphics-0.8.2 (14 Apr 2026)

- add `ComputeCommandEncoder::barrier()` for inline compute-to-compute synchronization within a pass
- enable `naga::valid::Capabilities::SUBGROUP` for shader validation
- vulkan: fall back to UBO for larger uniform blocks on all systems
- metal: enable fast math, skip debug groups in production

## blade-graphics-0.8.1 (28 Mar 2026)

- new API for enumerating device availability
- bumped the max pass count to 1000 for better ML compatibility

## blade-graphics-0.8, blade-util-0.3, blade-egui-0.7, blade-particle-0.1, blade-asset-0.2.1, blade-engine-0.1 (26 Mar 2026)

- examples:
  - moved some of the old example code into GPU tests
  - new "info" example to show supported GPUs
  - new Asteroids XR example 
- graphics:
  - OpenXR / Android support (tested on Quest 3S)
  - option to disable ray tracing initialization
  - separate `Capabilities` flag for binding arrays, including TLAS arrays
  - cooperative matrix operations support (auto-detected via `Capabilities`)
  - `wait_for` now returns `Result<bool, DeviceError>` instead of `bool`,
    distinguishing timeout from device-lost errors
  - `memory_stats()` API for querying VRAM budget/usage (via `VK_EXT_memory_budget`)
  - `Buffer::size()` accessor on all backends
    - debug bounds check on `BufferPiece::data()`
  - `PlatformError` is now a unified opaque type across all backends
  - `ComputePipelineBase` trait exposes `get_workgroup_size()` for generic code
  - `NotSupportedError`, `DeviceError`, and `PlatformError` implement `Display` + `Error`
  - vk: set `MUTABLE_FORMAT` on depth+stencil textures for flexible view creation
  - vk: graceful handling of surface acquire errors instead of panicking
  - vk: reject GPUs that cannot present in Intel+NVIDIA PRIME configurations
  - egl: use DMA-BUF sharing with different displays for presentation
  - vk: uniform buffer fallback for buggy Qualcomm devices
  - metal: fix lifetimes of acceleration structures
- particle:
  - new crate forged from the original particle example
- asset:
  - support procedural assets
- engine:
  - moved the engine from "blade" itself, reserving it for future use
  - choice between ray-tracing and rasterization rendering pipelines
  - first-class XR support

## blade-graphics-0.7.1 (22 Feb 2026)

- vk: make us compatible with Mesa's LavaPipe

## blade-egui-0.7 (21 Feb 2026)

- update to egui-0.33 and blade-graphics-0.7

## blade-graphics-0.7 (27 Sep 2025)

- graphics
  - pipeline constants API
  - allow buffer bindings for uniform data
  - supported MSAA samples are now returned in context `Capabilities`
  - Vulkan:
    - improve correctness of present synchronization

## blade-graphics-0.6, blade-util-0.2, blade-egui-0.6, blade-render-0.4, blade-0.3 (21 Dec 2024)

- graphics:
  - API for surface creation
    - allows multiple windows used by the same context
  - multi-sampling support
  - API for destruction of pipelines
  - return detailed initialization errors
  - every pass now takes a label
  - automatic GPU pass markers
  - ability to capture pass GPU timings
  - ability to force the use of a specific GPU
  - ability to set viewport
  - fragment shader is optional
  - support more texture formats
  - Metal:
    - migrate to "objc2"
    - support for workgroup memory
    - concurrent compute dispatches
  - Egl:
    - destroy old surface on resize
  - Vulkan:
    - support unused bind groups
- egui:
  - fix blending color space

## blade-egui-0.5 (09 Nov 2024)

- update egui to 0.29

## blade-graphics-0.5, blade-macros-0.3, blade-egui-0.4, blade-util-0.1 (27 Aug 2024)

- crate: `blade-util` for helper utilities
- graphics:
  - vertex buffers support
  - surface configuration:
    - transparency support
    - option to disable exclusive fullscreen
    - VK: using linear sRGB color space if available
  - exposed initialization errors
  - exposed device information
  - Vk:
    - fixed initial RAM consumption
    - worked around Intel descriptor memory allocation bug
    - fixed coherent memory requirements
    - rudimentary cleanup on destruction
  - GLES:
    - support for storage buffer and compute
    - scissor rects, able to run "particle" example
    - blending and draw masks
    - fixed texture uploads
- examples: "move"
- window API switched to raw-window-handle-0.6

## blade-graphics-0.4, blade-render-0.3, blade-0.2 (22 Mar 2024)

- crate: `blade` for high-level engine
  - built-in physics via Rapier3D
- examples: "vehicle"
- render:
  - support object motion
  - support clockwise mesh winding
  - fixed mipmap generation
- update to egui-0.26 and winit-0.29
- graphics:
  - display sync configuration
  - color space configuration
  - work around Intel+Nvidia presentation bug
  - overlay support

## blade-graphics-0.3, blade-render-0.2 (17 Nov 2023)

- tangent space generation
- spatio-temporal resampling
- SVGF de-noising
- environment map importance sampling
- shaders as assets
  - with includes, enums, and bitflags
  - with hot reloading
- load textures: `exr`, `hdr`
- utility: `FramePacer`
- examples: scene editing in "scene"
  - using egui-gizmo for manipulation

## blade-graphics-0.2, blade-render-0.1 (31 May 2023)

- ray tracing support
- examples: "ray-query", "scene"
- crate: `blade-egui` for egui integration
- crate: `blade-asset` for asset pipeline
- crate: `blade-render` for ray-traced renderer
  - load models: `gltf`
  - load textures: `png`, `jpg`

## blade-graphics-0.1 (25 Jan 2023)

- backends: Vulkan, Metal, OpenGL ES + WebGL2
- examples: "mini", "bunnymark", "particle"
- crate: `blade-graphics` for GPU abstracting GPU operations
- crate: `blade-macros` for `ShaderData` derivation
