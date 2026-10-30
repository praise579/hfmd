# 用 Rust 实现，不依赖 hf-hub

需要跨平台单二进制交付，且核心是「段状态机 + 校验 + 路径映射」这类分支密集的逻辑，因此选 Rust，用类型化的 `Result` 区分错误类别。

## Status

accepted。

2026-10-30 修订：原方案是「复用 `hf-hub` 的仓库 API 与缓存布局，只自建下载器」。实测后放弃该依赖。

## 放弃 hf-hub 的原因

- `BlobLfsInfo` 的字段叫 `sha256`，而 Hub API 返回的是 `lfs.oid`，没有 serde rename，**所有 LFS 文件的 sha256 恒为 `None`**。实测 `Qwen/Qwen2.5-7B-Instruct` 会退化成返回 git blob SHA-1，直接违反 ADR-0002 的 blob 命名与内容校验。
- `get_paths_info` 返回同一个结构体，无法绕开；`api_url` / `paginate` 是 `pub(crate)`，没有可复用的底层入口。
- 它的下载器是文件间并发，且不支持跨进程续传，本来就不在复用范围内。

于是改为自己发 `GET /api/models/<repo>/tree/<rev>?recursive=true&expand=true`，用自有结构体解析 `lfs.oid`，并跟随 `Link` 头的 `rel="next"` 分页。

## Considered Options

- **Go**：交叉编译更顺、贡献门槛更低，但没有官方 HF SDK，仓库 API 与缓存布局要从头写。
- **hf-hub + 对 LFS 文件逐个 HEAD 补 sha256**：代码最少，但每个 LFS 文件多一次请求，大仓库明显变慢。
- **接受只有 SHA-1**：放弃与 `transformers` 缓存互通，违背 ADR-0002。

## Consequences

- 需要自己维护 Hub API 的请求、鉴权与错误映射（`src/api.rs`），但这也正是下载器需要的同一层 HTTP 能力。
- 贡献门槛高于 Go，需要在 README 与 `CONTRIBUTING` 里写清如何跑测试与真机验证。
- 缓存布局不再有现成实现可借，完全由 hfmd 按 ADR-0002 实现。
