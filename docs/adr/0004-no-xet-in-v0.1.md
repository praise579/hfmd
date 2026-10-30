# v0.1 不集成 xet，统一走 HTTP Range

v0.1 不接入 `hf-xet`。所有仓库（包括带 `xetHash` 的）统一走 `.../resolve/<rev>/<path>` 加 HTTP `Range` 分片下载；xet 文件的请求会 302 到 CAS，仍按普通 HTTP 处理，校验优先使用 `lfs.oid`。

理由是 xet 自带块级并行与块级续传，与 hfmd 的段状态模型是两套引擎，同时维护会翻倍复杂度；而统一走 HTTP 能保证只有一条下载路径需要正确性验证。

## Consequences

- 纯 xet 仓库的下载速度可能不及官方 `hf`，README 中需注明这一点。
- 需要在文件信息里保留 `xetHash`，为将来接入留出位置；若某仓库的 `Range` 不可用，降级为单流下载并在日志中说明。
