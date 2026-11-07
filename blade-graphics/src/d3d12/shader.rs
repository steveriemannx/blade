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

    /// Compiles HLSL `source` for `entry` (e.g. `"main"`) and `target`
    /// (e.g. `"cs_6_0"`) into a DXIL container.
    ///
    /// On failure the [`Error::ShaderCompile`] payload carries the compiler
    /// diagnostics.
    pub fn compile_hlsl(&self, source: &str, entry: &str, target: &str) -> Result<Vec<u8>, Error> {
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
        let args_wide: Vec<Vec<u16>> = ["-E", entry, "-T", target]
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
