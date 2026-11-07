//! HLSL -> DXIL compilation through the runtime DXC toolchain.
//!
//! D3D12 consumes DXIL, which `naga` does not emit today. The phase 4 plan is
//! `naga` WGSL -> HLSL -> `dxcompiler.dll` -> DXIL, with DXIL compiled at
//! runtime so the pipeline cache can key on source and defines. This module is
//! the DXIL half: it loads `dxcompiler.dll` through the normal Windows DLL
//! search path and exposes [`ShaderCompiler::compile_hlsl`].
//!
//! `dxcompiler.dll` and `dxil.dll` are runtime dependencies. Put them next to
//! the executable, on `PATH`, or point `BLADE_DXC_PATH` at `dxcompiler.dll`.
//! `dxil.dll` must sit next to `dxcompiler.dll` so the compiler can sign the
//! DXIL: D3D12 rejects unsigned DXIL outside developer mode.

use std::ffi::{OsStr, OsString, c_void};
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::Graphics::Direct3D::Dxc::{
    CLSID_DxcCompiler, DXC_CP_UTF8, DXC_OUT_ERRORS, DXC_OUT_KIND, DXC_OUT_OBJECT, DxcBuffer,
    IDxcBlob, IDxcCompiler3, IDxcIncludeHandler, IDxcResult,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{GUID, HRESULT, Interface, PCWSTR, s};

use super::Error;

/// Entry point exported by `dxcompiler.dll`.
type DxcCreateInstanceProc =
    unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;

/// Environment variable holding a full path to `dxcompiler.dll`, for machines
/// that keep the DXC release outside the normal DLL search path.
pub const DXC_PATH_ENV: &str = "BLADE_DXC_PATH";

/// Minimal graphics shader pair: a fullscreen triangle whose pixel color comes
/// from a `float4` in the 32-bit root constants at `b0`.
pub const FULLSCREEN_HLSL: &str = r#"
cbuffer Params : register(b0) {
    float4 g_color;
};

struct VSOutput {
    float4 position : SV_Position;
};

VSOutput vs_main(uint vertex_id : SV_VertexID) {
    float2 uv = float2((vertex_id << 1) & 2, vertex_id & 2);
    VSOutput output;
    output.position = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return output;
}

float4 ps_main(VSOutput input) : SV_Target {
    return g_color;
}
"#;

/// Mesh shader pair used by tests and examples: per-vertex position and color,
/// interpolated color output. Position Z is the depth value used for depth
/// testing.
pub const MESH_HLSL: &str = r#"
struct VSInput {
    float3 position : POSITION;
    float3 color : COLOR;
};

struct VSOutput {
    float4 position : SV_Position;
    float3 color : COLOR;
};

VSOutput vs_main(VSInput input) {
    VSOutput output;
    output.position = float4(input.position, 1.0);
    output.color = input.color;
    return output;
}

float4 ps_main(VSOutput input) : SV_Target {
    return float4(input.color, 1.0);
}
"#;

/// Textured fullscreen quad: samples a bound `Texture2D` at `t0` with the
/// static sampler at `s0` and writes the result to `SV_Target`.
pub const TEXTURED_HLSL: &str = r#"
Texture2D g_texture : register(t0);
SamplerState g_sampler : register(s0);

struct VSOutput {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};

VSOutput vs_main(uint vertex_id : SV_VertexID) {
    float2 uv = float2((vertex_id << 1) & 2, vertex_id & 2);
    VSOutput output;
    output.position = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    output.uv = uv;
    return output;
}

float4 ps_main(VSOutput input) : SV_Target {
    return g_texture.Sample(g_sampler, input.uv);
}
"#;

/// Multiple-render-target shader: writes a different constant color to
/// `SV_Target0` and `SV_Target1`.
pub const MRT_HLSL: &str = r#"
struct VSOutput {
    float4 position : SV_Position;
};

struct PSOutput {
    float4 target0 : SV_Target0;
    float4 target1 : SV_Target1;
};

VSOutput vs_main(uint vertex_id : SV_VertexID) {
    float2 uv = float2((vertex_id << 1) & 2, vertex_id & 2);
    VSOutput output;
    output.position = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return output;
}

PSOutput ps_main(VSOutput input) {
    PSOutput output;
    output.target0 = float4(1.0, 0.0, 0.0, 1.0);
    output.target1 = float4(0.0, 1.0, 0.0, 1.0);
    return output;
}
"#;

/// WGSL version of the fullscreen-triangle shader pair used to exercise the
/// WGSL -> HLSL -> DXIL path.
pub const WGSL_FULLSCREEN: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vertex_index << 1u) & 2u), f32(vertex_index & 2u));
    return vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
}

@fragment
fn ps_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
"#;

/// WGSL textured quad: a uniform `float4` tint (root constants at `b0`) and a
/// texture at `t1` (group 0 binding 1), sampled with `textureLoad` so the
/// shader needs no sampler. Exercises the WGSL -> HLSL binding map.
///
/// Samplers are deliberately avoided: the workspace `naga` fork's HLSL backend
/// models samplers as a dynamically indexed heap (the `wgpu-hal` dx12 model),
/// which does not line up with this backend's static samplers yet. HLSL
/// entry points still cover sampled textures.
pub const WGSL_TEXTURED: &str = r#"
struct Params {
    color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var source: texture_2d<f32>;

struct VSOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VSOutput {
    let uv = vec2<f32>(f32((vertex_index << 1u) & 2u), f32(vertex_index & 2u));
    var output: VSOutput;
    output.position = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    output.uv = uv;
    return output;
}

@fragment
fn ps_main(input: VSOutput) -> @location(0) vec4<f32> {
    let dimensions = vec2<f32>(textureDimensions(source));
    let texel = textureLoad(source, vec2<i32>(input.uv * dimensions), 0);
    return texel * params.color;
}
"#;

/// DXR library: a ray-generation shader that traces one ray against the bound
/// top-level acceleration structure, plus miss and closest-hit shaders, writing
/// the hit distance into the output buffer.
pub const RAY_TRACING_LIBRARY_HLSL: &str = r#"
struct RayPayload {
    float hit_t;
};

RaytracingAccelerationStructure g_tlas : register(t0);
RWStructuredBuffer<float> g_output : register(u0);

[shader("raygeneration")]
void ray_gen() {
    RayDesc ray;
    ray.Origin = float3(0.0, 0.0, -2.0);
    ray.Direction = float3(0.0, 0.0, 1.0);
    ray.TMin = 0.0;
    ray.TMax = 100.0;

    RayPayload payload;
    payload.hit_t = -1.0;
    TraceRay(g_tlas, RAY_FLAG_NONE, 0xFF, 0, 0, 0, ray, payload);
    g_output[0] = payload.hit_t;
}

[shader("miss")]
void miss(inout RayPayload payload) {
    payload.hit_t = -1.0;
}

[shader("closesthit")]
void closest_hit(inout RayPayload payload, in BuiltInTriangleIntersectionAttributes attributes) {
    payload.hit_t = RayTCurrent();
}
"#;

/// Storage-texture compute shader: writes a gradient into an
/// `RWTexture2D<float4>` bound at `u0`, one texel per thread.
pub const COMPUTE_TEXTURE_WRITE_HLSL: &str = r#"
RWTexture2D<float4> g_target : register(u0);

[numthreads(4, 4, 1)]
void main(uint3 thread_id : SV_DispatchThreadID) {
    g_target[thread_id.xy] = float4(
        (float(thread_id.x) + 0.5) / 4.0,
        (float(thread_id.y) + 0.5) / 4.0,
        0.5,
        1.0
    );
}
"#;

/// Ray-query compute shader: traces one ray from `(0, 0, -2)` along `+Z` against
/// the bound top-level acceleration structure and writes the hit distance (or
/// `-1` for a miss) into the output buffer.
pub const RAY_QUERY_HLSL: &str = r#"
RaytracingAccelerationStructure g_tlas : register(t0);
RWStructuredBuffer<float> g_hit : register(u0);

[numthreads(1, 1, 1)]
void main() {
    RayDesc ray;
    ray.Origin = float3(0.0, 0.0, -2.0);
    ray.Direction = float3(0.0, 0.0, 1.0);
    ray.TMin = 0.0;
    ray.TMax = 100.0;

    RayQuery<RAY_FLAG_NONE> query;
    query.TraceRayInline(g_tlas, RAY_FLAG_NONE, 0xFF, ray);
    while (query.Proceed()) {}

    g_hit[0] = query.CommittedStatus() == COMMITTED_TRIANGLE_HIT ? query.CommittedRayT() : -1.0;
}
"#;

/// Trivial compute shader used by tests and examples: writes the dispatch
/// thread id into the bound output buffer.
pub const TRIVIAL_COMPUTE_HLSL: &str = r#"
RWStructuredBuffer<uint> g_output : register(u0);

[numthreads(64, 1, 1)]
void main(uint3 tid : SV_DispatchThreadID) {
    g_output[tid.x] = tid.x;
}
"#;

/// A runtime-loaded DXC HLSL compiler.
pub struct ShaderCompiler {
    module: HMODULE,
    // Taken before dropping the module: the vtable lives inside the DLL.
    compiler: Option<IDxcCompiler3>,
}

impl ShaderCompiler {
    /// Loads `dxcompiler.dll` and creates an `IDxcCompiler3`.
    pub fn load() -> Result<Self, Error> {
        let (module, create_instance) = load_dxc_library()?;
        let mut compiler: Option<IDxcCompiler3> = None;
        let hr = unsafe {
            create_instance(
                &CLSID_DxcCompiler,
                &IDxcCompiler3::IID,
                &mut compiler as *mut _ as *mut *mut c_void,
            )
        };
        if let Err(err) = hr.ok() {
            unsafe {
                let _ = FreeLibrary(module);
            }
            return Err(Error::Windows(err));
        }
        let Some(compiler) = compiler else {
            unsafe {
                let _ = FreeLibrary(module);
            }
            return Err(Error::ShaderToolchain(
                "DxcCreateInstance returned no IDxcCompiler3".to_string(),
            ));
        };
        Ok(Self {
            module,
            compiler: Some(compiler),
        })
    }

    /// Translates WGSL to HLSL for `entry`/`stage` without compiling it. Useful
    /// for debugging generated code.
    pub fn translate_wgsl(&self, wgsl: &str, entry: &str, stage: &str) -> Result<String, Error> {
        let stage = super::wgsl::shader_stage(stage).ok_or(Error::InvalidState(
            "unknown shader stage; expected vertex, fragment or compute",
        ))?;
        Ok(super::wgsl::wgsl_to_hlsl(wgsl, entry, stage)?.source)
    }

    /// Compiles WGSL to DXIL: WGSL -> HLSL (naga `hlsl-out`) -> DXIL (DXC).
    pub fn compile_wgsl(&self, wgsl: &str, entry: &str, stage: &str) -> Result<Vec<u8>, Error> {
        let stage = super::wgsl::shader_stage(stage).ok_or(Error::InvalidState(
            "unknown shader stage; expected vertex, fragment or compute",
        ))?;
        let translation = super::wgsl::wgsl_to_hlsl(wgsl, entry, stage)?;
        let target = super::wgsl::stage_target(stage);
        self.compile_hlsl(&translation.source, &translation.entry_point, target)
    }

    /// Compiles HLSL `source` for `entry` (e.g. `"main"`) and `target`
    /// (e.g. `"cs_6_0"`) into a DXIL container.
    ///
    /// On failure the [`Error::ShaderCompile`] payload carries the compiler
    /// diagnostics.
    pub fn compile_hlsl(&self, source: &str, entry: &str, target: &str) -> Result<Vec<u8>, Error> {
        self.compile_hlsl_with_args(source, &["-E", entry, "-T", target])
    }

    /// Compiles an HLSL library (e.g. `lib_6_3` for DXR state objects), where
    /// exports are declared with `[shader("...")]` attributes instead of `-E`.
    pub fn compile_hlsl_library(&self, source: &str, target: &str) -> Result<Vec<u8>, Error> {
        self.compile_hlsl_with_args(source, &["-T", target])
    }

    fn compile_hlsl_with_args(&self, source: &str, dxc_args: &[&str]) -> Result<Vec<u8>, Error> {
        let Some(compiler) = &self.compiler else {
            return Err(Error::ShaderToolchain(
                "shader compiler was already dropped".to_string(),
            ));
        };

        let buffer = DxcBuffer {
            Ptr: source.as_ptr() as *const c_void,
            Size: source.len(),
            Encoding: DXC_CP_UTF8.0,
        };

        // DXC takes wide-string arguments; keep the backing buffers alive for
        // the duration of the call.
        let args_wide: Vec<Vec<u16>> = dxc_args
            .iter()
            .map(|arg| {
                OsStr::new(arg)
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect()
            })
            .collect();
        let args: Vec<PCWSTR> = args_wide.iter().map(|arg| PCWSTR(arg.as_ptr())).collect();

        let result: IDxcResult =
            unsafe { compiler.Compile(&buffer, Some(&args), None::<&IDxcIncludeHandler>) }?;

        let status = unsafe { result.GetStatus()? };
        if let Err(err) = status.ok() {
            let diagnostics = read_output(&result, DXC_OUT_ERRORS)
                .map(|bytes| String::from_utf8_lossy(&bytes).trim_end().to_string())
                .unwrap_or_else(|| "no compiler diagnostics".to_string());
            return Err(Error::ShaderCompile(format!("{err}\n{diagnostics}")));
        }

        let dxil = read_output(&result, DXC_OUT_OBJECT)
            .ok_or_else(|| Error::ShaderCompile("DXC produced no object output".to_string()))?;
        Ok(dxil)
    }
}

impl Drop for ShaderCompiler {
    fn drop(&mut self) {
        // Drop the COM object before unloading the DLL that hosts its vtable.
        self.compiler.take();
        unsafe {
            let _ = FreeLibrary(self.module);
        }
    }
}

fn load_dxc_library() -> Result<(HMODULE, DxcCreateInstanceProc), Error> {
    let mut candidates: Vec<OsString> = Vec::new();
    if let Some(path) = std::env::var_os(DXC_PATH_ENV) {
        candidates.push(path);
    }
    candidates.push(OsString::from("dxcompiler.dll"));

    let mut last_error = String::from("no candidate path");
    for candidate in candidates {
        let wide: Vec<u16> = candidate.encode_wide().chain(std::iter::once(0)).collect();
        match unsafe { LoadLibraryW(PCWSTR(wide.as_ptr())) } {
            Ok(module) => {
                let Some(proc) = (unsafe { GetProcAddress(module, s!("DxcCreateInstance")) })
                else {
                    unsafe {
                        let _ = FreeLibrary(module);
                    }
                    last_error = format!(
                        "{} exports no DxcCreateInstance",
                        candidate.to_string_lossy()
                    );
                    continue;
                };
                // `GetProcAddress` yields an opaque function pointer; the DXC
                // entry point has a fixed signature.
                let create_instance: DxcCreateInstanceProc =
                    unsafe { std::mem::transmute(proc as *const c_void) };
                return Ok((module, create_instance));
            }
            Err(err) => {
                last_error = format!("{}: {err}", candidate.to_string_lossy());
            }
        }
    }

    Err(Error::ShaderToolchain(format!(
        "failed to load dxcompiler.dll ({last_error}); set {DXC_PATH_ENV} to its full path"
    )))
}

fn read_output(result: &IDxcResult, kind: DXC_OUT_KIND) -> Option<Vec<u8>> {
    let mut blob: Option<IDxcBlob> = None;
    unsafe { result.GetOutput(kind, ptr::null_mut(), &mut blob).ok()? };
    let blob = blob?;
    let size = unsafe { blob.GetBufferSize() };
    let data = unsafe { blob.GetBufferPointer() } as *const u8;
    if data.is_null() || size == 0 {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(data, size) }.to_vec())
}
