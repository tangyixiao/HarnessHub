# F23 — HF 模型资产 + 打包态 Python 资源定位（v0.5.0）

> 本文件是 F23 跨语言契约的**唯一事实来源**。规格源：`HARNESS_HUB_PROJECT_PLAN.md`
> §4.8 / §8.5；ROADMAP 阶段 23；CONTEXT.md「尚未实现」的打包态 Python 资源定位项。

## 0. 目标与非目标

**目标**（§8.5 六项 + CONTEXT 遗留项）：

1. HF repo metadata —— 显式触发的元数据查询（REST API，stdlib）。
2. Cache discovery —— 扫描本地 HF 缓存，只读。
3. Explicit download —— 仅用户显式发起；带 revision pinning 与磁盘预估。
4. Revision pinning —— 下载/信息按 revision 记录，不假设 `main`。
5. Local runtime capability API —— `models.status` 报告可选 SDK 可用性（探测，
   不 import；transformers/torch **永不**进依赖清单、永不 import）。
6. 打包态 Python 运行时资源定位与 `PYTHONPATH` 注入（F20 遗留项，并入本阶段）。

**非目标**：transformers 推理执行、量化、GPU 调优、模型评分/推荐；LangChain。

## 1. 架构归属

- Python `harness_hub_runtime/models/` 拥有网络（元数据/下载）与本地缓存扫描；
  Rust 拥有 IPC DTO、预算确认门、薄命令；前端 Settings 卡显式触发 + 磁盘警示。
- **零新增第三方依赖**：`huggingface_hub` 保持 optional（lazy find_spec 探测），
  元数据与缓存扫描用 stdlib `urllib.request` + 纯文件系统实现；下载在 SDK 缺失时
  退化为 stdlib 单文件直下（`resolve/<revision>/<file>` 直链，不支持 resume 的
  大文件场景如实标注 `via: "stdlib"`）。

## 2. 安全红线

1. **网络显式**：`models.status` / `models.cache.list` 零网络；网络只发生在
   `models.info` / `models.download.start` 这两个用户显式动作上。
2. **写路径锁死**：下载只允许落在 HF 缓存根（`HF_HOME`/`HUGGINGFACE_HUB_CACHE`
   env 优先，否则 `~/.cache/huggingface/hub`）或 `HARNESS_HUB_MODELS_ROOT` 显式
   覆盖；任何其他路径一律 `-32602` 拒绝。目标目录路径由 Rust 下发前校验前缀。
3. **磁盘预算门**：预估 > 2 GiB 的下载必须带 `confirmedDiskBudget: true`，否则
   返回 `{ needsConfirmation: true, estimatedBytes }`（对齐 AGENTS.md 2 GiB
   规划默认）。预估来源：`models.info` 的 siblings 大小求和；拿不到大小按未知
   处理（不虚报）。
4. **原文红线**：新增方法过 `_reject_raw_content`；`repo_id` 匹配
   `^[A-Za-z0-9._/-]+$`，revision 匹配 `^[A-Za-z0-9._/-]+$`（防路径穿越与 header
   注入；拒绝 `..` 段）。
5. **secret 不落库**：HF token 由用户环境变量（`HF_TOKEN`）继承进 sidecar 进程，
   Rust 不读取、不存储、不回显。

## 3. 跨语言契约（冻结）

### 3.1 RPC 方法表扩展

`SUPPORTED_CAPABILITIES` 追加 `"models.assets"`；`DEFAULT_METHODS` 追加：

```text
"models.status"   "models.info"   "models.cache.list"
"models.download.start"   "models.download.progress"   "models.download.cancel"
```

### 3.2 models.status

`{}` → `{ "sdk": { "huggingfaceHub": bool }, "cacheRoot": "<path>",
"downloads": [ { "repoId", "revision", "state", "via" } ],
"protocolVersion": "1.0" }`。`state` ∈ `idle|preparing|downloading|completed|failed|cancelled`。

### 3.3 models.info（网络，显式）

参数 `{ "repoId": "...", "revision": "..." }` →
`{ "repoId", "revision", "sizeBytes": <int|null>, "fileCount": <int>,
"files": [ { "path", "sizeBytes" } ] (≤64 项，超出截断 + "filesTruncated": true),
"gated": bool, "via": "api" }`。REST 端点：
`https://huggingface.co/api/models/<repo_id>/revision/<revision>`（stdlib urllib，
10s 超时）。失败 → JSON-RPC `-32004` + 短摘要（不回显响应体）。

### 3.4 models.cache.list

`{}` → `{ "entries": [ { "repoId", "revisions": [...], "sizeBytes": <int>,
"lastUsedAt": <rfc3339|null> } ] (≤128), "cacheRoot": "<path>" }`。
扫描规则：`models--<org>--<name>` 目录；refs/ 下每个文件是一个 revision 快照；
snapshots/ 子目录大小按文件求和（单仓上限 20000 文件，超出如实截断）。

### 3.5 models.download.start

参数 `{ "repoId", "revision", "files"?: [whitelist 文件名], "confirmedDiskBudget":
bool }` → 立即返回 `{ "downloadId", "state": "preparing" }`，后台线程执行；预算门
（§2.3）不满足时返回 `{ "needsConfirmation": true, "estimatedBytes": N }`。

- 同一 `(repoId, revision)` 已有进行中下载 → `-32005`。
- 进度查询 `models.download.progress` `{ "downloadId" }` →
  `{ "repoId", "revision", "state", "bytesDone", "bytesTotal" (int|null),
"fileDone", "fileCount", "currentFile", "lastError" (≤256) }`。
- `models.download.cancel` `{ "downloadId" }` → 幂等置 cancelled；进行中的文件
  写临时名 `.part`，取消/失败即删除，绝不留半截文件冒充完整资产。
- SDK 在场时用 `huggingface_hub.snapshot_download`（`allow_patterns` =
  files 白名单）；缺失走 stdlib 直下。`via` ∈ `"huggingface_hub" | "stdlib"`。
- 下载完成后登记进 cache（§3.4 同一扫描即见），**不新增 SQLite 表**（资产状态 =
  文件系统事实 + 进程内下载状态机；跨重启下载任务不恢复，UI 如实标注）。

### 3.6 Rust 侧

`src-tauri/src/sidecar/models.rs`：`ModelsStatus` / `ModelAssetEntry` /
`ModelDownloadProgress` DTO（camelCase + serde 契约测试）+
`decode_models_status` / `decode_model_cache_entries` /
`decode_download_progress` fail-closed decoder（坏形状收敛、字符串截断、数组上限，
对齐 F21 `decode_gateway_status` 风格）。薄命令：

```text
models_status / models_cache_list / models_info / models_download_start
models_download_progress / models_download_cancel
```

全部不返回 IPC Err（sidecar 未就绪收敛为 `unavailable*` 默认值）。download 命令
sidecar 未就绪时直接返回 `{ state: "unavailable" }`。

### 3.7 前端

`src/lib/ipc.ts`：`normalizeModelsStatus` / `normalizeModelCacheEntries` /
`normalizeModelDownloadProgress`（fixture test 锁形状与安全默认）。
`src/features/settings/ModelAssetsCard.tsx`：状态行（SDK 可用性、缓存根）、缓存
清单（只读）、下载表单（repoId + revision + 可选文件名白名单），预估 > 2 GiB 时
confirm 文案含字节数与「下载走网络、目标为 HF 缓存」明示；下载中显示进度轮询
（1s 间隔，仅在卡片展开时轮询）。组件只 import `@/lib/ipc`。

## 4. 打包态 Python 资源定位（S4）

`sidecar/config.rs::from_env` 的 runtime_root 缺省逻辑扩展（**dev 行为不变**）：

1. env 覆盖不变（`HARNESS_HUB_PYTHON` / `HARNESS_HUB_RUNTIME_ROOT` 优先）。
2. 新增探测顺序（仅当缺省开发路径 `<repo>/python` 不存在时）：exe 同级
   `resources/python/` → exe 上级 `resources/python/`。命中条件 =
   目录存在且其下有 `harness_hub_runtime/__init__.py`。
3. python 解释器：env 覆盖后若仍为裸 `"python"` 且探测到
   `resources/python/python.exe`（Windows）/ `resources/python/bin/python3`（其他）
   则用之。
4. 常量与探测函数可单测（注入根路径参数，不读全局 env）；真机打包验收项记录在
   release notes（dev 环境无法证伪打包形态时如实标注）。

## 5. 切片 / PR / 合并顺序

| 切片             | 分支                       | 文件域                                                                                   | 依赖                       |
| ---------------- | -------------------------- | ---------------------------------------------------------------------------------------- | -------------------------- |
| S1 Python models | `feat/f23/py-model-assets` | `python/harness_hub_runtime/models/**`、`rpc/server.py`、`python/tests/test_models_*.py` | F22 S1 已合入（server.py） |
| S2 Rust DTO+命令 | `feat/f23/rust`            | `src-tauri/src/sidecar/models.rs`、`commands.rs`、契约测试                               | 契约按 §3 冻结             |
| S3 前端          | `feat/f23/web`             | `src/lib/ipc.ts`、`ModelAssetsCard.tsx`、fixture tests                                   | S2 契约冻结                |
| S4 打包定位      | 并入 S2 分支               | `sidecar/config.rs` + 单测                                                               | 无                         |

合并顺序：S1 → S2（含 S4）→ S3 → docs。

## 6. 验收门

每切片各自门（同 F22 §7）；F23 全部合入后与 F22 一起跑 AGENTS.md 全门。
`pnpm python:test` 必须包含：协议层单元测试（fake transport 注入）、SSE 解析
fixture、缓存扫描临时目录用例、预算门用例、SDK 缺失降级用例。
