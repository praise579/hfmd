# hfmd

Hugging Face Modules Downloader —— 从 `huggingface.co`（或任意镜像）下载模型的跨平台命令行工具。

三条核心能力：

- **多线程**：文件间并发 × 文件内分片并发
- **断点续传**：段级、跨进程，`Ctrl-C` 之后接着下
- **完整性校验**：按内容哈希校验，不只是比大小

> 状态：v0.1 已实现 `download` / `info` / `cache`，尚未正式发版。规格见 [`.scratch/hfmd/spec.md`](.scratch/hfmd/spec.md)。

## 安装

从 [Releases](https://github.com/praise579/hfmd/releases) 下载对应平台的压缩包，解压后把 `hfmd` 放进 `PATH`。每个 release 附带 `SHA256SUMS` 供校验。

也可以从源码安装：

```bash
cargo install --git https://github.com/praise579/hfmd --locked
```

Homebrew / Scoop 的 formula 模板在 [`packaging/`](packaging/) 下，需要配套的 tap / bucket 仓库才能直接安装。

## 用法

```bash
# 下载整个仓库（默认写入 ~/.cache/huggingface/hub）
hfmd download Qwen/Qwen2.5-7B-Instruct

# 只下 safetensors，走镜像，单文件 8 连接
HF_ENDPOINT=https://hf-mirror.com hfmd download Qwen/Qwen2.5-7B-Instruct \
  --include "*.safetensors" --connections 8

# 先看会下什么
hfmd download Qwen/Qwen2.5-7B-Instruct --include "*.safetensors" --dry-run

# 查看与清理缓存
hfmd cache list
hfmd cache remove Qwen/Qwen2.5-7B-Instruct --yes
```

`--json` 可用于脚本；`-q` 只输出错误。中断后重新运行同一条命令即可从断点继续。


## 设计文档

- [`GLOSSARY.md`](GLOSSARY.md) —— 术语表
- [`docs/adr/`](docs/adr/) —— 架构决策记录
- [`.scratch/hfmd/spec.md`](.scratch/hfmd/spec.md) —— 命令面、行为规格与验收闸门

## 与现有工具的关系

`hfmd` 兼容 Hugging Face 标准缓存布局，下载结果可被 `transformers` / `vLLM` 直接加载。与官方 `hf` 和基于 aria2 的 `hfd` 相比，差异在于跨进程的段级续传与内容哈希校验——两者都不具备。

## 协议

MIT OR Apache-2.0
