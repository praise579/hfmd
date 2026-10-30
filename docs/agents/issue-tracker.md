# Issue 跟踪：GitHub

本仓库的 issue 与 spec 以 GitHub Issues 形式存在。所有操作使用 `gh` CLI。

仓库由 `git remote -v` 推断，在仓库内运行 `gh` 时会自动识别。

## 约定

- **创建 issue**：`gh issue create --title "..." --body "..."`，多行正文用 heredoc。
- **读取 issue**：`gh issue view <number> --json number,title,body,labels,comments`。
- **列出 issue**：`gh issue list --state open --json number,title,body,labels,comments --jq '[.[] | {number, title, body, labels: [.labels[].name], comments: [.comments[].body]}]'`，配合 `--label` 与 `--state` 过滤。
- **设子 issue**：`gh issue create --parent <parent> ...`，或事后 `gh issue edit <parent> --add-sub-issue <child>`（需 `gh` 2.94+）。旧版改用 `gh api --method POST repos/<owner>/<repo>/issues/<parent>/sub_issues -F sub_issue_id=<child-db-id>`。若子 issue 不可用，在子 issue 正文顶部写 `Part of #<parent>`。
- **阻塞关系**：使用 GitHub 原生依赖。`gh issue create --blocked-by <n>,<n> ...` 或 `gh issue edit <child> --add-blocked-by <blocker>`（`gh` 2.94+）。旧版：`gh api --method POST repos/<owner>/<repo>/issues/<child>/dependencies/blocked_by -F issue_id=<blocker-db-id>`，其中 `<blocker-db-id>` 是阻塞方的数字数据库 ID（`gh api repos/<owner>/<repo>/issues/<n> --jq .id`），不是 `#number` 或 `node_id`。两者都不可用时，在子 issue 正文顶部写 `Blocked by: #<n>, #<n>`。所有阻塞方关闭即视为解除。
- **统计未关闭的阻塞方**：`gh api repos/<owner>/<repo>/issues/<n> --jq .issue_dependencies_summary.blocked_by`（仅统计未关闭的，0 表示可以开工）。`gh issue view <n> --json blockedBy` 会一并列出已关闭的阻塞方，需过滤：`--jq '[.blockedBy.nodes[] | select(.state == "OPEN")] | length'`。
- **评论**：`gh issue comment <number> --body "..."`。
- **增删标签**：`gh issue edit <number> --add-label "..."` / `--remove-label "..."`。
- **关闭**：先用 `gh issue comment` 说明原因，再 `gh issue close <number>`。对已关闭的 issue 使用 `gh issue close --comment` 会丢弃评论但仍返回 0。

只在本项目自己的仓库里建 issue，不要提交到上游或他人仓库。在 fork 中 `gh` 可能默认指向父仓库，因此每次会话第一次 `gh issue create` 前，用 `gh repo view --json nameWithOwner` 确认目标。

## Pull request 作为请求入口

**PRs as a request surface: no.** _（若本仓库把外部 PR 视为功能请求，改为 `yes`；`/triage` 会读取这个标记。）_

设为 `yes` 时，PR 与 issue 走同一套标签与状态，对应命令换成 `gh pr` 系列：

- 读取 PR：`gh pr view <number>`，再看 `gh pr view <number> --comments`；diff 用 `gh pr diff <number>`。
- 列出待 triage 的外部 PR：`gh api --paginate 'repos/{owner}/{repo}/pulls?state=open' --jq '.[] | select(.author_association | IN("OWNER","MEMBER","COLLABORATOR") | not) | {number, title, author: .user.login, author_association, labels: [.labels[].name]}'`。
- 评论 / 打标签 / 关闭：`gh pr comment`、`gh pr edit --add-label`/`--remove-label`、`gh pr close`。

GitHub 的 issue 与 PR 共用一个编号空间，裸 `#42` 可能是其中之一：先用 `gh pr view 42` 解析，失败再退回 `gh issue view 42`。

## 当 skill 说 "publish to the issue tracker"

创建一个 GitHub issue。

## 当 skill 说 "fetch the relevant ticket"

按上文 **读取 issue** 的方式读取。

## Wayfinding 操作

供 `/wayfinder` 使用。**map** 是一个 issue，**子 issue** 是 ticket。

- **Map**：一个带 `wayfinder:map` 标签的 issue，承载 Notes / Decisions-so-far / Fog 正文。`gh issue create --label wayfinder:map`。
- **子 ticket**：通过 GitHub 子 issue 关联到 map（见上）。若子 issue 不可用，在 map 正文里用任务列表列出子 issue，并在子 issue 正文顶部写 `Part of #<map>`。标签为 `wayfinder:<type>`（`research`/`prototype`/`grilling`/`task`）。认领后分配给认领者。
- **阻塞**：见上文 **阻塞关系**。
- **Frontier 查询**：列出 map 下未关闭的子 issue，剔除有未关闭阻塞方或已有 assignee 的，按 map 中的顺序取第一个。
- **认领**：`gh issue edit <n> --add-assignee @me`。
- **完成**：`gh issue comment <n> --body "<答案>"` → `gh issue close <n>` → 在 map 的 Decisions-so-far 追加一条上下文指针。
