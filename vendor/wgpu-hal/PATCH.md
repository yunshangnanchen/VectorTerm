# wgpu-hal 本地改动说明

基线：`wgpu-hal 24.0.4`（crates.io 原版，从 `~/.cargo/registry/src/index.crates.io-*/wgpu-hal-24.0.4` 复制）。
`Cargo.toml` 里用 `[patch.crates-io]` 指向本目录。

升级 wgpu 时**不要直接覆盖本目录**：先看下面这处改动，重新在新版本里处理完再换。

## 改动一：交换链 Scaling / SwapEffect 环境变量钩子

`src/dx12/mod.rs` 的 `Surface::configure`，建交换链那一段（原版是 `Scaling: Dxgi::DXGI_SCALING_STRETCH` 硬编码）：

```rust
let scaling = match std::env::var("VECTORTERM_DX_SCALING").as_deref() {
    Ok("none") => Dxgi::DXGI_SCALING_NONE,
    _ => Dxgi::DXGI_SCALING_STRETCH,
};
let effect = match std::env::var("VECTORTERM_DX_EFFECT").as_deref() {
    Ok("sequential") => Dxgi::DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    _ => Dxgi::DXGI_SWAP_EFFECT_FLIP_DISCARD,
};
```

另外两处 `eprintln!` 是诊断输出：本工程没有接 `log`，`log::error!` 会被丢掉，
所以交换链创建 / `ResizeBuffers` 失败的原因直接打到 stderr。

**不设环境变量时行为与原版完全一致。**

### 为什么留这个钩子（改动一）

`docs/resize-fix-plan.md` 的 L1 方案要把 `DXGI_SCALING_STRETCH` 换成 `DXGI_SCALING_NONE`
来消除拖动缩放时的拉伸抖动。有了这个钩子，验证这件事不需要改代码、不需要重新编译：

```bash
VECTORTERM_DX_SCALING=none ./target/release/VectorTerm.exe
```

`VECTORTERM_DX_EFFECT` 是配套的对照组（`FLIP_SEQUENTIAL` + `NONE` 是 MSDN 明确支持的组合）。

## 改动二：逐像素 alpha（合成交换链）

两处，配合 DirectComposition 使用（见 `docs/custom-background.md` 第 5.1 节）：

1. `src/auxil/dxgi/conv.rs` 的 `map_acomposite_alpha_mode()` 原本把一切映射成
   `DXGI_ALPHA_MODE_IGNORE`，现在如实映射 `PreMultiplied` / `STRAIGHT` / `IGNORE`。
2. `src/dx12/adapter.rs` 的 `surface_capabilities`：`WndHandle` 目标只报
   `Opaque`（hwnd 交换链确实不支持预乘 alpha），
   `Visual` / `SurfaceHandle` / `SwapChainPanel` 目标才报 `PreMultiplied`。

关键点：**能不能用预乘 alpha，取决于 surface target 而不是参数组合**。

`CreateSwapChainForHwnd` 对 `DXGI_ALPHA_MODE_PREMULTIPLIED` 一律返回 `0x887A0001`
（`Scaling` / `SwapEffect` 四种组合都试过，与它们无关），所以 hwnd 目标必须只报
`Opaque`，否则 wgpu 会挑一个创建必然失败的配置。

走 `SurfaceTargetUnsafe::CompositionVisual(visual_ptr)`（`CreateSwapChainForComposition`）
时预乘 alpha 才被接受，此时窗口还必须带 `WS_EX_NOREDIRECTIONBITMAP`
（**创建窗口时就要给**，事后 `SetWindowLongW` 无效），并且每帧 `Present` 之后要补一次
`IDCompositionDevice::Commit`。

早期曾把这两处改动还原过，因为当时只试了 hwnd 交换链，得出"这条路不通"的结论——
换成合成交换链后才成立。
