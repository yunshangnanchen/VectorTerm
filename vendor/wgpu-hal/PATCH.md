# wgpu-hal 本地改动说明

基线：`wgpu-hal 24.0.4`（crates.io 原版，从 `~/.cargo/registry/src/index.crates.io-*/wgpu-hal-24.0.4` 复制）。
`Cargo.toml` 里用 `[patch.crates-io]` 指向本目录。

升级 wgpu 时**不要直接覆盖本目录**：先看下面这处改动，重新在新版本里处理完再换。

## 唯一的改动

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

## 为什么留这个钩子

`docs/resize-fix-plan.md` 的 L1 方案要把 `DXGI_SCALING_STRETCH` 换成 `DXGI_SCALING_NONE`
来消除拖动缩放时的拉伸抖动。有了这个钩子，验证这件事不需要改代码、不需要重新编译：

```bash
VECTORTERM_DX_SCALING=none ./target/release/VectorTerm.exe
```

`VECTORTERM_DX_EFFECT` 是配套的对照组（`FLIP_SEQUENTIAL` + `NONE` 是 MSDN 明确支持的组合）。

## 顺带记一笔：不要在这里改 alpha 模式

曾经在这个 crate 里把 `composite_alpha_modes` 加进 `PreMultiplied` 并让
`map_acomposite_alpha_mode()` 如实映射，想让透明背景生效。结论是**这条路不通**：
`CreateSwapChainForHwnd` 对 `DXGI_ALPHA_MODE_PREMULTIPLIED` 一律返回 `0x887A0001`，
跟 `Scaling` / `SwapEffect` 无关（四种组合都试过）。两处改动已还原成原版。
半透明改用层叠窗口统一 alpha 实现，见 `docs/custom-background.md` 第 5 节。
