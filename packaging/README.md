# 打包模板

这两个文件是发布后填写用的模板，**不会随仓库自动发布**：

- `homebrew/hfmd.rb` —— Homebrew formula，需要放进一个 `homebrew-tap` 仓库（`brew tap <owner>/tap`）才能 `brew install`
- `scoop/hfmd.json` —— Scoop manifest，需要放进一个 bucket 仓库才能 `scoop install`

发布 v* tag 后，从 Release 页的 `SHA256SUMS` 里取对应平台的哈希，替换文件中的 `REPLACE_WITH_SHA256`，并把 `version` 改成实际版本。

`winget` 需要向 [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs) 提交 PR，无法仅靠本仓库自动化。
