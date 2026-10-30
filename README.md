# hfmd

Hugging Face Modules Downloader —— 从 `huggingface.co`（或任意镜像）下载模型的跨平台命令行工具。

三条核心能力：

- **多线程**：文件间并发 × 文件内分片并发
- **断点续传**：段级、跨进程，`Ctrl-C` 之后接着下
- **完整性校验**：按内容哈希校验，不只是比大小

> 状态：v0.1 处于设计与规格阶段，尚未发布。规格见 [`.scratch/hfmd/spec.md`](.scratch/hfmd/spec.md)。

## 设计文档

- [`GLOSSARY.md`](GLOSSARY.md) —— 术语表
- [`docs/adr/`](docs/adr/) —— 架构决策记录
- [`.scratch/hfmd/spec.md`](.scratch/hfmd/spec.md) —— 命令面、行为规格与验收闸门

## 与现有工具的关系

`hfmd` 兼容 Hugging Face 标准缓存布局，下载结果可被 `transformers` / `vLLM` 直接加载。与官方 `hf` 和基于 aria2 的 `hfd` 相比，差异在于跨进程的段级续传与内容哈希校验——两者都不具备。

## 协议

MIT OR Apache-2.0
