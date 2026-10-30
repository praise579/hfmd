# hfmd

从 Hugging Face 下载模型的跨平台命令行工具。本上下文描述仓库内容、本地缓存的磁盘布局，以及一次下载从分片到校验的全过程。

## Language

### 仓库内容

**仓库 (Repository)**:
Hugging Face 上一个带命名空间的模型、数据集或 Space，标识为 `<namespace>/<name>`。
_Avoid_: 模型库、项目、repo（口语可用，文档中统一用「仓库」）

**Revision**:
指向仓库某一次提交的名字。可以是 commit hash，也可以是分支或 tag 名。
_Avoid_: 版本号、标签

**Blob**:
按内容命名的实体文件。LFS 文件以其内容 sha256 命名，普通文件以 git blob SHA-1 命名。
_Avoid_: 实体文件、缓存文件、数据文件

**Snapshot**:
某个 revision 下的文件视图，由一组指向 blob 的相对符号链接组成。
_Avoid_: 快照目录、版本目录

**Ref**:
分支或 tag 名到 commit hash 的映射，保存在仓库缓存目录的 `refs/` 下。
_Avoid_: 指针、引用文件

**Endpoint**:
HF API 与文件下载的基地址。官方站与镜像都是 endpoint，二者只在地址上有区别。
_Avoid_: 服务器、镜像地址、代理

### 下载过程

**分片 (Segment)**:
文件内一段连续、可独立下载与续传的字节区间。分片是数据概念，与运行时使用多少线程或连接无关。
_Avoid_: 块、chunk、线程、连接

**段状态 (Segment state)**:
一个未完成文件中每个分片已写入的偏移，持久化后可跨进程恢复。
_Avoid_: 进度文件、断点、临时文件

**未完成文件 (Incomplete file)**:
正在下载、尚未重命名为 blob 的文件。
_Avoid_: 半成品、临时文件

**完整性校验 (Verification)**:
用仓库声明的内容哈希比对下载结果。LFS 文件用 sha256，普通文件用 git blob SHA-1，并与声明的大小一并比对。
_Avoid_: hash 校验、校验和、size 校验
