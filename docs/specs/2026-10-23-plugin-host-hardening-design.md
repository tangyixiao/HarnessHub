# 插件宿主边界修复

- 日期：2026-10-23
- 关联：插件宿主 v0.1、ADR-0018、ADR-0011
- 范围：现有 stderr 诊断缓冲与 Usage 导入入口，沿用 PR #13。

## stderr 的资源边界

现有按行读取会在换行前无限积累数据；环形缓冲只淘汰完整旧行，因此一条超长行仍能超过承诺的 64 KiB。

改为固定大小字节块读取，并只保存最后 64 KiB 原始字节。无换行、非法 UTF-8 与多字节字符跨块均不能中断读取或扩大缓存。显示末尾若干行时才以 UTF-8 容错解码；`max_lines = 0` 返回空字符串。原始字节上限不等于解码后字符串的字节数上限。

## Usage 导入身份与降级

已登记插件的 `id` 与 `path` 是导入入口的身份依据。调用方目录 canonicalize 后必须与登记目录相同，manifest 的 `id` 必须与请求的插件 id 相同；验证在启动子进程前完成。错配返回 `InvalidInput`，不得写入 Usage 数据，也不得把健康的已登记插件标记为 degraded。

匹配身份后，spawn、握手、调用、响应形状或 Usage 解析失败均将该插件标记为 degraded；再次调用直接返回 `PluginDegraded`，显式 refresh 才可恢复。SQLite 自身的导入失败不改变插件状态。合法空结果仍成功且不降级。

不改变 SQLite schema、IPC DTO、capability 协商、observe-only 决策或外部 Harness 数据。

## 验证

stderr 以单测锁定字节上限、末尾内容和行选择，并以真 Python 进程验证无换行/非法编码输出。Usage 测试从空数据库开始，仅调用生产 refresh 建立登记；目录或 id 错配不得启动错误插件或写入事件；调用失败须可观察到 degraded，refresh 后恢复。所有修复须先记录 RED，再实现并跑全部完成门。
