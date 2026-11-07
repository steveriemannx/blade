# Licensing

The code in this repository is licensed under the MIT license (see
[`../LICENSE`](../LICENSE)). Third-party crates stay under their own licenses;
this document records the compatibility policy and the known exceptions.

## Policy

- Prefer dependencies that are MIT or dual-licensed as MIT/Apache-2.0 (or
  another permissive pair including MIT). Dual-licensed crates are consumed
  under their MIT terms.
- Avoid **Apache-2.0-only** dependencies: when an optional feature pulls one,
  drop the feature instead of the dependency (the Wayland client-side
  decoration feature of `winit` is the known example).
- Unavoidable Apache-2.0-only crates remain acceptable, but must be visible in
  the audit below and recorded in [`CHANGELOG.md`](CHANGELOG.md).

## Known decisions

- **winit** is used with `default-features = false` plus `rwh_06`, `x11`,
  `wayland` and `wayland-dlopen`. The default `wayland-csd-adwaita` feature
  pulls `sctk-adwaita` -> `ab_glyph` -> `ab_glyph_rasterizer` /
  `owned_ttf_parser`, all Apache-2.0-only. Consequence: Wayland windows get no
  client-side decorations; macOS and Windows are unaffected.
- **D3D12**: `blade-graphics`'s Windows-only `d3d12` feature uses the `windows`
  crate (MIT/Apache-2.0, target-gated, optional) and `naga`'s `hlsl-out`
  feature (the workspace `naga` fork is MIT/Apache-2.0). DXIL compilation calls
  `dxcompiler.dll` at runtime (Microsoft's DirectXShaderCompiler, MIT); no
  compiler binary is vendored.
- The project's own source and examples are MIT; there are no `LICENSE` files
  with a different license inside the crates.

## Audit

List the license of every package in the workspace (with network access, or a
fully populated cargo cache):

```sh
cargo metadata --format-version 1 --locked \
  | python3 -c '
import json, sys
allowed = ("MIT", "BSD", "ISC", "Zlib", "0BSD", "Unicode")
for package in sorted(json.load(sys.stdin)["packages"], key=lambda p: p["name"]):
    license = package.get("license") or "UNKNOWN"
    if not any(token in license for token in allowed):
        print(f"{package[\"name\"]} {package[\"version\"]}: {license}")
'
```

The command prints only the packages without an obvious MIT-compatible license
string; review them and either drop the feature, replace the crate, or record
the exception here. A CI gate (`cargo-deny` with a `deny.toml`) is the natural
next step once the list is stable.
