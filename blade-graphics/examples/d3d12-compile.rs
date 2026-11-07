//! Compile HLSL to DXIL with the runtime DXC toolchain.
//!
//! Needs `dxcompiler.dll` + `dxil.dll` (DXC release) next to the executable, on
//! `PATH`, or at `BLADE_DXC_PATH`. Windows-only, requires the `d3d12` feature.
//!
//! Run on Windows:
//! `cargo run -p blade-graphics --features d3d12 --example d3d12-compile`

fn main() {
    #[cfg(windows)]
    {
        use blade_graphics::d3d12::{ShaderCompiler, shader};

        let compiler = match ShaderCompiler::load() {
            Ok(compiler) => compiler,
            Err(err) => {
                eprintln!("failed to load DXC: {err}");
                eprintln!(
                    "hint: set {}=<path>\\dxcompiler.dll (keep dxil.dll next to it)",
                    shader::DXC_PATH_ENV
                );
                std::process::exit(1);
            }
        };

        let dxil = match compiler.compile_hlsl(shader::TRIVIAL_COMPUTE_HLSL, "main", "cs_6_0") {
            Ok(dxil) => dxil,
            Err(err) => {
                eprintln!("shader compilation failed: {err}");
                std::process::exit(1);
            }
        };
        println!(
            "compiled cs_6_0: {} bytes, magic: {:?}",
            dxil.len(),
            String::from_utf8_lossy(&dxil[..4.min(dxil.len())])
        );

        // The error path is part of the contract: diagnostics must surface.
        match compiler.compile_hlsl("this is not hlsl", "main", "cs_6_0") {
            Ok(_) => {
                eprintln!("BUG: invalid HLSL compiled successfully");
                std::process::exit(1);
            }
            Err(err) => {
                let text = err.to_string();
                let first_line = text.lines().find(|line| !line.is_empty()).unwrap_or("");
                println!("invalid HLSL rejected with diagnostics: {first_line}");
            }
        }

        println!("DXC HLSL -> DXIL: OK");
    }

    #[cfg(not(windows))]
    println!("d3d12-compile is Windows-only");
}
