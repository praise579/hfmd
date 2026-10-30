# 领域文档

工程类 skill 在探索本仓库代码库时应如何消费领域文档。

## 探索之前先读这些

- 仓库根目录的 **`GLOSSARY.md`**，或者
- 根目录的 **`GLOSSARY-MAP.md`**（若存在）：它指向每个上下文各自的一份 `GLOSSARY.md`，
  读取与当前主题相关的每一份。
- **`docs/adr/`**：读取与即将改动的领域相关的 ADR。

任一文件不存在时，**静默继续**。不要提示缺失，也不要主动建议创建。
`/domain-modeling` skill（经由 `/grill-with-docs` 和 `/improve-codebase-architecture` 触达）
会在术语或决策真正落地时按需创建它们。

## 文件结构

单上下文仓库（绝大多数仓库）：

```
/
├── GLOSSARY.md
├── docs/adr/
│   ├── 0001-event-sourced-orders.md
│   └── 0002-postgres-for-write-model.md
└── src/
```

多上下文仓库（根目录存在 `GLOSSARY-MAP.md`）：

```
/
├── GLOSSARY-MAP.md
├── docs/adr/                          ← 系统级决策
└── src/
    ├── ordering/
    │   ├── GLOSSARY.md
    │   └── docs/adr/                  ← 上下文级决策
    └── billing/
        ├── GLOSSARY.md
        └── docs/adr/
```

本仓库目前为**单上下文**。

## 使用术语表里的词汇

当输出涉及某个领域概念（issue 标题、重构方案、假设、测试名）时，使用 `GLOSSARY.md`
中定义的术语。不要漂移到术语表明确回避的同义词。

若需要的概念还不在术语表中，这是一个信号：要么你在发明项目不使用的语言（应重新考虑），
要么存在真实缺口（记下来交给 `/domain-modeling`）。

## 标记 ADR 冲突

若输出与已有 ADR 矛盾，显式指出，而不是悄悄覆盖：

> _与 ADR-0007（event-sourced orders）矛盾，但仍值得重开，因为…_
