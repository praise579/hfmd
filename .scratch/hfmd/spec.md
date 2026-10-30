# hfmd v0.1 Spec

## 目标

从 `huggingface.co`（及其他 endpoint）下载模型的跨平台命令行工具，具备三项能力：

1. **多线程**：文件间并发 × 文件内分片并发
2. **断点续传**：段级、跨进程
3. **完整性校验**：按内容哈希校验，而非只比大小

定位为生产级开源替代品：Rust 实现，MIT OR Apache-2.0，三平台发布。

## 非目标（v0.1 明确不做）

- `upload`、`login`、`jobs`、`scan` 子命令
- `--local-dir` 平铺模式（只走标准缓存布局）
- `hf-xet` 块级并行（见 ADR-0004）
- 主站与镜像之间的自动切换

## 命令面

```
hfmd download <repo_id> [文件...]
    --revision <rev>        默认 refs/main
    --include <glob>        可重复
    --exclude <glob>        可重复
    --endpoint <url>
    --token <token>
    --jobs <n>              文件间并发，默认 4
    --connections <n>       每文件分片并发，默认 8
    --max-retries <n>
    --skip-verify
    --dry-run               只列出将下载的文件与总量
    --json                  结构化输出
    -q, --quiet

hfmd info <repo_id> [--revision <rev>] [--endpoint <url>]
hfmd cache list [--json]
hfmd cache remove <repo_id> [--revision <rev>]
```

## 行为规格

### Endpoint 与鉴权

优先级：命令行参数 > 环境变量 > 配置文件 > 默认值。

- endpoint：`--endpoint` > `HF_ENDPOINT` > `~/.config/hfmd/config.toml` > `https://huggingface.co`
- token：`--token` > `HF_TOKEN` > `~/.cache/huggingface/token`

`hf-hub` 自身不读环境变量，以上链路必须由 hfmd 解析后显式传给客户端。

### 磁盘布局

标准 HF 缓存布局（见 ADR-0002）：

- blob 名：LFS 文件用 `lfs.oid`（sha256），普通文件用 `oid`（git blob SHA-1）
- snapshot 目录：commit hash，内部为指向 `../../blobs/<name>` 的相对符号链接
- `refs/<branch>`：commit hash，仅在整批文件下载并校验通过后更新
- Windows 退化为硬链接，硬链接失败才复制，并记录降级日志

### 分片与并发

- 片大小目标 16–64 MiB，按文件大小计算片数并受 `--connections` 封顶
- 小于 8 MiB 的文件单流下载，不切分
- 文件间并发由 `--jobs` 控制

### 断点续传

每个未完成文件维护一份段状态（`[start, end, offset]` 列表），原子写入（临时文件 + rename）。重启后逐片续传。段状态与未完成文件不得落入 `snapshots/`。

同一仓库同一时间只允许一个进程下载：获取仓库级文件锁，失败时提示「已有 hfmd 在下载该仓库」并退出。

### 完整性校验

- LFS 文件：内容 sha256 比对 `lfs.oid`
- 非 LFS 文件：git blob SHA-1 比对 `oid`，同时比对大小
- 校验失败：删除该文件的未完成数据与段状态，重新下载；重试耗尽则整次运行失败并保留其他文件的断点
- `--skip-verify` 跳过校验，日志中明确标注未校验
- 日志中注明每个文件实际使用的校验算法

### 错误与重试

- 可重试：超时、连接中断、5xx、429 —— 指数退避 + 抖动
- 不可重试：401/403（提示需要 token 或权限不足）、404（文件不存在）
- 失败信息给出可操作提示，而不是原始状态码

### 输出

- 默认：人类可读的进度条与最终汇总
- `--json`：稳定的结构化输出，供脚本消费
- 结构化日志，错误分类可被 grep

## 验收闸门

v0.1 发布前必须同时满足：

1. 真实下载一个 10 GB 级模型（如 Qwen2.5-7B safetensors 全量）
2. 中途 `Ctrl-C` 后重启，各分片从断点接续，且总下载字节数小于全量
3. 全部文件校验通过，`transformers` 能直接加载
4. `cache list` / `cache remove` 可用，remove 后不残留段状态
5. 三平台（Linux x86_64/aarch64、macOS arm64/x86_64、Windows x64）CI 绿
6. README 含安装方式（Releases + brew/scoop/winget）、用法与已知限制（xet 速度）

## 决策索引

完整决策记录见 `docs/adr/`、术语见根目录 `GLOSSARY.md`。

| 决策 | 结论 |
|---|---|
| 语言 | Rust，`hf-hub` 只做 API 与缓存路径（ADR-0001） |
| 缓存布局 | 兼容官方布局，Windows 退化硬链接（ADR-0002） |
| 续传 | 段级、跨进程、自建状态文件（ADR-0003） |
| xet | v0.1 不集成（ADR-0004） |
| 镜像 | 三级覆盖，不自动回退 |
| 分片 | 自适应 16–64 MiB，<8 MiB 单流 |
| 校验 | 分级：LFS sha256 / 普通 SHA-1 |
| 重试 | 分类重试 + 指数退避 |
| 协议 | MIT OR Apache-2.0 |
| 多进程 | 仓库级文件锁 |

## 待办

- [x] 创建 GitHub 仓库并把 issue 跟踪切回 GitHub Issues —— https://github.com/praise579/hfmd ，13 个 issue 已建并设置阻塞关系
- [ ] 真机验证 xet 仓库的 `Range` 可用性（影响 ADR-0004 的降级分支）
