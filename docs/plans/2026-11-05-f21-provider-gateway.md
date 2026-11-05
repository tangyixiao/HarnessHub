# F21 Provider Gateway + Tokenizer — 实施计划

- 日期：2026-11-05
- 冻结契约：`docs/specs/2026-11-05-f21-provider-gateway-design.md`（**唯一事实来源**）
- 路线图：`docs/ROADMAP.md` §F21
- 模式：加速模式（多子 agent 并行、小 Task、多 Conventional Commit、每切片独立 PR 并合并、goal 自动续做）

---

## 前置确认（已核实，2026-11-05）

| 项 | 结论 |
|----|------|
| 真实远端 `main` | `6d6c327`（`git fetch https://... '+refs/heads/*:refs/remotes/origin/*' --prune` 后 `origin/main` = `6d6c327`；`git diff --stat HEAD origin/main` 为空） |
| 集成分支 | `feat/f21-provider-gateway`（HEAD = `6d6c327`，已 push 到远端） |
| 本地 `main` 标签 | 落后 83 个提交，**禁止**作为 diff / merge 基线；一律用 `6d6c327` / `feat/f21-provider-gateway` |
| git 通道 | SSH 被 `198.18.0.125:443` 拦截 → 一律 HTTPS + `gh`；`gh auth setup-git` 已配置 credential helper；禁止 `http.extraHeader` |
| commit 易被吞 | 每次 commit 后 `git log -1 --oneline` 复核 |
| Python 测试 | `python -m unittest discover -s python/tests -t python -v` 直接可跑（25 tests OK），无需 uv/venv |
| 工具链 | node/pnpm/vitest/eslint/prettier（主检出 `node_modules`）、cargo（`src-tauri/target`）、python 3.14 全部就绪 |
| 遗留产物 | `build-release-040/` 未跟踪，**绝不提交** |
| 质量门 | `pnpm format:check && pnpm rust:fmt:check` / `lint` / `typecheck` / `test && rust:test && python:test` / `build` / `rust:clippy` |

---

## 目标与验收映射

| 验收（spec §1.2） | 责任切片 | 证据 |
|-------------------|----------|------|
| 1 F20 零回归 | 全部 | `python:test` / `rust:test` / `test` 全绿 + F20 用例数不减 |
| 2 Python 契约层 TDD + 零依赖 | S1 / S2 | `test_providers.py` / `test_tokenization.py`，RED 证据 |
| 3 Rust DTO 序列化契约测试 | S4 | `gateway.rs` 内联 `#[cfg(test)]` |
| 4 TS normalize fixture | S5 | `ipc.test.ts` |
| 5 features/* 不直接 invoke | S5 | `ProviderGatewayCard` 只 import `@/lib/ipc` |
| 6 gateway_status 永不 IPC Err | S4 / S5 | 命令测试 + 前端 fail-closed fixture |
| 7 全质量门真实输出 | S6 | 本文件「完成门记录」 |
| 8 零新增依赖 | 全部 | `pyproject.toml` / `package.json` / `Cargo.toml` 不变 |
| 9 local-first | S1 / S3 | 原文拒绝闸 + 不回显原始对象测试 |

---

## 提交 / PR 切分（写作用域两两互斥）

| 切片 | 分支 | 独占写作用域 | PR base |
|------|------|--------------|---------|
| S0 | `feat/f21-provider-gateway` | `docs/specs/2026-11-05-f21-*`、`docs/plans/2026-11-05-f21-*` | —（直接提交到集成分支） |
| S1 | `f21/py-providers` | `python/harness_hub_runtime/providers/**`、`python/tests/test_providers.py` | `feat/f21-provider-gateway` |
| S2 | `f21/py-tokenization` | `python/harness_hub_runtime/tokenization/**`、`python/tests/test_tokenization.py` | 同上 |
| S3 | `f21/py-rpc` | `python/harness_hub_runtime/rpc/server.py`、`python/tests/test_rpc_server.py` | 同上 |
| S4 | `f21/rust` | `src-tauri/src/sidecar/gateway.rs`、`mod.rs`、`config.rs`、`src-tauri/src/commands.rs`、`src-tauri/src/lib.rs` | 同上 |
| S5 | `f21/web` | `src/lib/ipc.ts`、`src/lib/ipc.test.ts`、`src/features/settings/ProviderGatewayCard.tsx(.test.tsx)`、`SettingsPage.tsx` | 同上 |
| S6 | `feat/f21-provider-gateway` | 质量门修复 + 本文件「完成门记录」+ 最终 PR → `main` | — |

> S1–S5 在各自 git worktree 中并行开发；每个切片自己 `git commit`（提交后 `git log -1` 复核），
> **由 Lead 集中执行 push / `gh pr create` / `gh pr merge`**，避免并发 push 竞争 credential helper。
> 合并顺序按依赖友好排序：S1 → S2 → S3 → S4 → S5。

---

## 任务分解（每切片 TDD：先 RED 提交，再 GREEN 提交）

### S0 — 冻结契约（Lead，串行，最先）
- `Task 0.1` `docs: 新增 F21 跨语言契约 spec 与实施计划`
- 冻结 §3.2 静态列表、§3.3 `gateway.status` 形状、§5 四级 precedence、§6.1 DTO 键名、§7.1 TS 类型。

### S1 — `test:` RED + `feat:` providers 契约层
- `Task 1.1` `test: providers DTO / streaming / error / route / lazy 的失败用例`
  （断言 §4.1 冻结 API、`to_json()` 键、§4.2/§4.3 映射表、§3.5 路由规则、`probe()` 恒不抛）
- `Task 1.2` `feat: 实现 providers 契约层（零依赖）`
  文件：`dto.py` / `streaming.py` / `errors.py` / `routing.py` / `lazy_import.py` + 重写 `__init__.py`
- `Task 1.3`（可选）`refactor:` 消除重复，保持 API 不变

### S2 — `test:` RED + `feat:` tokenization 契约层
- `Task 2.1` `test: truth precedence / estimate / revision 绑定的失败用例`
  （含 `select_truth({"local_exact":7,"estimated":9}) == ("local_exact",7)` 证明 precedence 逻辑）
- `Task 2.2` `feat: 实现 tokenization 契约层（零依赖）`
  文件：`truth.py` / `estimate.py` / `revision.py` + 重写 `__init__.py`

### S3 — `test:` RED + `feat:` RPC 方法 + 能力协商
- `Task 3.1` `test: 更新 F20 常量 fixture 并新增 F21 RPC 契约用例`
  **必须同提交更新** `test_rpc_server.py` 的 `CAPABILITIES` / `METHODS` 常量（§3.2），
  并新增：`runtime.info`/`initialize` 反映新列表、`gateway.status` 全形状、
  `gateway.route` 原文拒绝闸、F20 三项 requiredCapabilities 仍通过、shutdown 语义不回归。
- `Task 3.2` `feat: 扩展 server.py 方法表与能力协商（lazy 优雅降级）`
  `gateway.status` **自包含**：静态声明 + 可选 SDK 探测（缺模块时静态回退，恒 `state="ready"`）；
  其余方法 lazy-import S1/S2 模块，缺模块时返回合法降级形状（合并后自动升级为真实实现）。

### S4 — `test:` RED + `feat:` Rust 契约层
- `Task 4.1` `test: GatewayStatus 序列化 / decoder / 薄命令的失败用例`
- `Task 4.2` `feat: GatewayStatus DTO + 安全 decoder + gateway_status 命令 + handler 注册`
  新增 `sidecar/gateway.rs`；`mod.rs` 加 `pub mod gateway;`；`config.rs` **不改** REQUIRED_CAPABILITIES；
  `commands.rs` 加薄命令 + 契约测试；`lib.rs` generate_handler 字母序插入。

### S5 — `test:` RED + `feat:` 前端契约层
- `Task 5.1` `test: normalizeGatewayStatus fixture + ProviderGatewayCard 失败用例`
- `Task 5.2` `feat: normalizeGatewayStatus + gatewayStatus() + ProviderGatewayCard`
  `ipc.ts` 追加（不改动 F20 既有导出）；`ipc.test.ts` 追加；新卡片文件；`SettingsPage.tsx` 追加
  `<ProviderGatewayCard />` 一行 + import。

### S6 — 集成质量门 + 最终 PR（Lead，串行）
- `Task 6.1` 合并全部切片 PR，处理跨切片集成问题（若有）
- `Task 6.2` 跑全质量门，贴真实输出，填入「完成门记录」
- `Task 6.3` `docs: 记录 F21 完成门真实输出` + 最终 PR `feat/f21-provider-gateway → main` 并合并
- `Task 6.4` `goal complete`

---

## 并行编排细节

```
主检出  D:\HarnessHub                         分支 feat/f21-provider-gateway（含 S0）
worktree D:\tmp\dsh-temp\f21-wt\s1           分支 f21/py-providers
worktree D:\tmp\dsh-temp\f21-wt\s2           分支 f21/py-tokenization
worktree D:\tmp\dsh-temp\f21-wt\s3           分支 f21/py-rpc
worktree D:\tmp\dsh-temp\f21-wt\s4           分支 f21/rust
worktree D:\tmp\dsh-temp\f21-wt\s5           分支 f21/web
```

- 每个 worktree 从 `feat/f21-provider-gateway`（S0 提交）创建，自带冻结 spec 全文。
- **S5 worktree** 建 `node_modules` junction 指向 `D:\HarnessHub\node_modules`（Windows `New-Item -ItemType Junction`），
  以便 `vitest run`；其余 worktree 不需要。
- **S4 worktree** 设 `CARGO_TARGET_DIR=D:\HarnessHub\src-tauri\target` 复用热缓存（唯一 Rust 切片，无并发冲突）。
- Python 切片（S1/S2/S3）直接 `python -m unittest discover -s python/tests -t python -v`。
- 子 agent 只做「写码 + 跑本切片测试 + commit」；**不 push、不开 PR**。
- 各切片测试命令：
  - S1: `python -m unittest discover -s python/tests -p test_providers.py -t python -v`
  - S2: `python -m unittest discover -s python/tests -p test_tokenization.py -t python -v`
  - S3: `python -m unittest discover -s python/tests -t python -v`（含 F20 全量回归）
  - S4: `cargo test --manifest-path src-tauri/Cargo.toml sidecar::gateway`（+ 最终全量由 Lead 跑）
  - S5: `pnpm exec vitest run src/lib/ipc.test.ts src/features/settings/ProviderGatewayCard.test.tsx`

---

## 完成门记录（S6 填入真实输出，未执行前保持「待执行」）

| 门 | 命令 | 结果 | 真实输出摘要 |
|----|------|------|--------------|
| format | `pnpm format:check && pnpm rust:fmt:check` | 待执行 | 待执行 |
| lint | `pnpm lint` | 待执行 | 待执行 |
| typecheck | `pnpm typecheck` | 待执行 | 待执行 |
| unit tests | `pnpm test && pnpm rust:test && pnpm python:test` | 待执行 | 待执行 |
| build | `pnpm build` | 待执行 | 待执行 |
| clippy | `pnpm rust:clippy` | 待执行 | 待执行 |

> 报告时贴真实 stdout/stderr，不复述 AGENTS.md 的期望值；既有 flake / 警告需单独标注。

---

## 风险与对策

| 风险 | 对策 |
|------|------|
| S3 的 worktree 无 providers/tokenization 包 → import 失败 | spec §2.1 / §10：静态声明与动态就绪度分离；`gateway.status` 自包含 + 其余方法 lazy 优雅降级；合并后自动升级 |
| S5 worktree 缺 node_modules | 预建 junction |
| 并发 push 竞争 credential helper | 子 agent 不 push；Lead 串行 push / PR / merge |
| Windows 删除被拦截（worktree 清理） | 合并完成后用 `git worktree remove`，失败则保留（不影响交付） |
| commit 被吞 | 每个切片 commit 后 `git log -1 --oneline` 复核，Lead 再核一次 |
| PTY / 进程树测试在 Windows CI 偶发 flake | F20 已记录为既有 flake；复现则单独复跑并标注，不算 F21 回归 |
| 误把 `build-release-040/` 提交 | `git add` 只用明确路径，禁止 `git add -A` |
