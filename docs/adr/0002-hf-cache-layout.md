# 兼容 Hugging Face 标准缓存布局

缓存目录读写官方布局 `~/.cache/huggingface/hub/models--<org>--<repo>/{blobs,snapshots,refs}`：blob 按内容哈希命名（LFS 用 `lfs.oid`，普通文件用 git blob SHA-1），snapshot 目录名为 commit hash，内部是指向 `../../blobs/<name>` 的相对符号链接，`refs/main` 存 commit hash。

这样下载完成的内容可被 `transformers` / `vLLM` 直接加载，多个 revision 共享同一份 blob 不重复占盘，也能复用用户已有的 HF 缓存。

## Consequences

- Windows 无符号链接权限时退化为硬链接，硬链接不可用才复制，并在日志中明确告知降级。
- 未完成文件与段状态必须放在不会污染 `snapshots/` 的位置，且 `refs/` 只在整批文件下载并校验通过后才更新，避免出现「ref 指向不完整 snapshot」。
