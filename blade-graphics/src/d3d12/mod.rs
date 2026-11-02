//! Minimal Direct3D 12 initialization (phase 0).
//!
//! This module is deliberately standalone: it creates a DXGI factory, picks an
//! adapter that can host a feature level 12_0 `ID3D12Device`, and creates one
//! direct command queue. It does **not** implement the HAL traits yet, and it is
//! not wired into `blade-graphics`'s backend selection.
//!
//! See [`blade-graphics/etc/d3d12.md`](../../etc/d3d12.md) for the full
//! design and the phased plan.
//!
//! Enabled by the `d3d12` cargo feature on Windows only.

use std::fmt;

use windows::core::Error as WinError;
use windows::Win32::Graphics::Direct3D::{D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_12_0};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_COMMAND_QUEUE_FLAG_NONE,
    D3D12_COMMAND_QUEUE_PRIORITY_NORMAL, D3D12CreateDevice, ID3D12CommandQueue, ID3D12Device,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_CREATE_FACTORY_FLAGS, DXGI_ERROR_NOT_FOUND,
    DXGI_GPU_PREFERENCE, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE, IDXGIAdapter1, IDXGIFactory6,
};

/// Minimum feature level the backend targets. Blade's Vulkan backend already
/// requires 1.3-era features (dynamic rendering, descriptor indexing, ray
/// queries), which D3D12 only matches at 12_0 + recent drivers.
pub const MIN_FEATURE_LEVEL: D3D_FEATURE_LEVEL = D3D_FEATURE_LEVEL_12_0;

/// Properties of a DXGI adapter, reported for diagnostics.
#[derive(Clone, Debug)]
pub struct AdapterInfo {
    /// Adapter description, e.g. `"NVIDIA GeForce RTX 4070"`.
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub dedicated_video_memory: u64,
    pub shared_system_memory: u64,
    /// True for the software rasterizer (WARP).
    pub is_software: bool,
}

/// Result of a successful [`init`] call.
pub struct Init {
    pub factory: IDXGIFactory6,
    pub adapter: IDXGIAdapter1,
    pub device: ID3D12Device,
    pub queue: ID3D12CommandQueue,
    pub adapter_info: AdapterInfo,
    pub node_count: u32,
}

/// Why Direct3D 12 initialization failed.
#[derive(Debug)]
pub enum InitError {
    /// No DXGI adapter could be enumerated.
    NoAdapter,
    /// Adapters exist, but none supports a feature level 12_0 D3D12 device.
    NoCompatibleDevice,
    /// A DXGI/D3D12 call failed.
    Windows(WinError),
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter => f.write_str("no DXGI adapter found"),
            Self::NoCompatibleDevice => {
                f.write_str("no adapter supports a feature level 12_0 D3D12 device")
            }
            Self::Windows(err) => write!(f, "D3D12 initialization failed: {err}"),
        }
    }
}

impl std::error::Error for InitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Windows(err) => Some(err),
            _ => None,
        }
    }
}

impl From<WinError> for InitError {
    fn from(err: WinError) -> Self {
        Self::Windows(err)
    }
}

/// Creates a DXGI factory, a D3D12 device on the most preferred adapter, and
/// one direct command queue.
pub fn init() -> Result<Init, InitError> {
    init_with_preference(DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE)
}

/// Like [`init`], but with an explicit adapter preference
/// (see `DXGI_GPU_PREFERENCE_*`).
pub fn init_with_preference(preference: DXGI_GPU_PREFERENCE) -> Result<Init, InitError> {
    let factory: IDXGIFactory6 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };

    let mut saw_any_adapter = false;
    let mut index = 0;
    loop {
        let adapter: IDXGIAdapter1 =
            match unsafe { factory.EnumAdapterByGpuPreference(index, preference) } {
                Ok(adapter) => adapter,
                Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(err) => return Err(err.into()),
            };
        index += 1;
        saw_any_adapter = true;

        // Skip adapters that cannot host a D3D12 device of the minimum level.
        let mut device: Option<ID3D12Device> = None;
        if let Err(err) = unsafe { D3D12CreateDevice(&adapter, MIN_FEATURE_LEVEL, &mut device) } {
            log::debug!("D3D12: adapter {index} rejected: {err}");
            continue;
        }
        let Some(device) = device else {
            log::debug!("D3D12: adapter {index} returned no device");
            continue;
        };

        let adapter_info = describe_adapter(&adapter)?;
        let queue_desc = D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            Priority: D3D12_COMMAND_QUEUE_PRIORITY_NORMAL.0,
            Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
            NodeMask: 0,
        };
        let queue: ID3D12CommandQueue = unsafe { device.CreateCommandQueue(&queue_desc) }?;
        let node_count = unsafe { device.GetNodeCount() };

        log::info!(
            "D3D12 device initialized: \"{}\" (vendor 0x{:04X}, device 0x{:04X}), \
             dedicated video memory {} MiB, software: {}, nodes: {node_count}",
            adapter_info.name,
            adapter_info.vendor_id,
            adapter_info.device_id,
            adapter_info.dedicated_video_memory / (1024 * 1024),
            adapter_info.is_software,
        );

        return Ok(Init {
            factory,
            adapter,
            device,
            queue,
            adapter_info,
            node_count,
        });
    }

    if saw_any_adapter {
        Err(InitError::NoCompatibleDevice)
    } else {
        Err(InitError::NoAdapter)
    }
}

fn describe_adapter(adapter: &IDXGIAdapter1) -> Result<AdapterInfo, InitError> {
    let desc = unsafe { adapter.GetDesc1() }?;
    let name_len = desc
        .Description
        .iter()
        .position(|&ch| ch == 0)
        .unwrap_or(desc.Description.len());
    Ok(AdapterInfo {
        name: String::from_utf16_lossy(&desc.Description[..name_len]),
        vendor_id: desc.VendorId,
        device_id: desc.DeviceId,
        dedicated_video_memory: desc.DedicatedVideoMemory as u64,
        shared_system_memory: desc.SharedSystemMemory as u64,
        is_software: desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
    })
}
