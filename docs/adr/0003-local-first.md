# ADR-0003 — Local-first 是默认约束，不是可选配置

- **Status**: Accepted（2026-04）
- **Context**: 我们会读取用户的 Prompt、模型响应、代码 diff、Session 日志与 Git 活动。这些是最
  敏感的一类数据。任何"默认上传/默认联网"的设计都会让产品在安全评审中直接被否。
- **Decision**:
  1. 原始 Prompt / Response / Code / Session Log **默认不上传**。
  2. 统计、索引、搜索默认全部在本机完成（SQLite + 本地索引）。
  3. 网络功能必须**显式开启、可关闭、可解释**（用户能说出这一条请求为什么发出去）。
  4. Usage 只能估算时必须标记 `estimated`，不得伪装成账单级精度。
  5. Secret 不进入主 SQLite，只保存 `credential_ref` 句柄，真实值交给 OS 密钥链。
- **Alternatives**:
  - 云端聚合统计（跨设备看板）：产品价值明显，但与本地优先原则冲突；推迟到 v0.2 之后并必须 opt-in。
  - 匿名遥测默认开启：直接否定。
- **Consequences**:
  - 所有"跨设备/团队"特性都要显式设计并默认关闭。
  - 数据层必须支持导出（HHAR），避免用户被 SQLite 锁死。
  - 需要为"本地估算"与"上游精确值"设计来源优先级（见 ADR-0008）。
- **Evidence**: `docs/CONTEXT.md` 核心约束 1；`AGENTS.md` 禁止事项；`src-tauri/src/db/` 只落本地文件。
- **Revisit Conditions**: 只有在用户明确要求且默认关闭的前提下，才考虑引入任何上传路径。
