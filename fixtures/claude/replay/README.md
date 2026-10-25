# Claude Code replay fixtures

## 取证记录（2026-10-25，B6）

- 来源：本机 `~/.claude/projects/**/*.jsonl` 的**键名级**取证（只看结构与 type 枚举，
  未复制任何用户内容）；内容全部合成（UUID 为合成 UUID v4 形态、文本为示例文案）。
- 已取证的行形状：
  - 顶层公共键：`parentUuid` / `isSidechain` / `type` / `uuid` / `timestamp` /
    `userType` / `entrypoint` / `cwd` / `sessionId` / `version` / `gitBranch`
  - `user`：`message.role="user"`，`message.content` 为**字符串**或
    `[{type:"text",text}]` 数组（sidechain 里见过数组形态）。
  - `assistant`：`message` 含 `model` / `content[]` / `stop_reason` / `usage`；
    `content[]` 元素见过的 `type`：`text` / `thinking` / `tool_use`。
  - 其它已见 `type`：`permission-mode` / `file-history-snapshot` / `attachment` /
    `summary`（B6 全部计入 skipped，不解析内容）。
- `basic.jsonl` 刻意包含：字符串与数组两种 user content、text+thinking 组合、
  tool_use（unsupported 计数）、sidechain 一对消息、未知类型行、坏行（非 JSON）、
  summary 行——用于锁定解析器的容错契约。

## 夹具表

| 文件          | 用途                                             |
| ------------- | ------------------------------------------------ |
| basic.jsonl   | 解析器全形状 + 容错（未知类型 / 坏行 / sidechain） |

## 纪律

- 绝不把真实用户 Prompt / 回答 / 路径放进本目录；新增夹具先在本文件登记取证来源。
