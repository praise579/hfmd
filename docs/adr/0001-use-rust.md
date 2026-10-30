# 用 Rust 实现，hf-hub 只做 API 与缓存布局

需要跨平台单二进制交付，且核心是「段状态机 + 校验 + 路径映射」这类分支密集的逻辑，因此选 Rust，用类型化的 `Result` 区分错误类别。

`hf-hub` crate 只用于仓库文件列表、revision 解析、鉴权与缓存路径计算；**不复用它的下载器**——它做的是文件间并发，临时文件按进程 UUID 命名，重跑从头下载，没有跨进程续传能力。

## Considered Options

- **Go**：交叉编译更顺、贡献门槛更低，但没有官方 HF SDK，仓库 API 与缓存布局要从头写约 1–2k 行。
- **双原型对比**：更稳妥，但要多花几天，且四个关键事实（`list_tree` 能力、endpoint 支持、下载器不可扩展、blob 命名规则）已经查清，收益不足以抵消时间成本。

## Consequences

- 贡献门槛高于 Go，需要在 README 与 `CONTRIBUTING` 里写清如何跑测试与真机验证。
- `hf-hub` 不读环境变量，`HF_ENDPOINT` / `HF_TOKEN` 的优先级链必须自己实现。
