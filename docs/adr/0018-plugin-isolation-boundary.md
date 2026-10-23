# ADR-0018 — 插件核心隔离：out-of-process 默认、受限 Host API、故障不外溢

- **Status**: Accepted（2026-10-17，Phase 9.3 插件宿主 v0.1）
- **Sources**:
  - `HARNESS_HUB_PROJECT_PLAN.md` 的「ADR-0018 — Plugin Core Isolation」（草案两句话）
  - `HARNESS_HUB_PROJECT_PLAN.md` 9.3 插件 SDK 六项
  - 本仓库 `plugins/README.md:10` 与 `src-tauri/src/plugins/mod.rs:4` 早已引用本 ADR，
    但 `docs/adr/` 里一直没有落文件 —— 本 ADR 补上这个锚点
  - 设计规格：`docs/specs/2026-10-16-plugins-host-design.md`

- **Context**:

  计划书草案只给了两条结论句（「第三方插件默认 out-of-process」「插件不得直接访问主数据库和
  Secret Store，只能通过受限 Host API」），没有说清楚三件事：

  1. **为什么**必须 out-of-process，而不是 in-process 的 trait object + 谨慎调用；
  2. 受限 Host API 的**边界怎么落**——v0.1 到底拦截还是只记录；
  3. 插件崩溃/挂起时宿主的行为契约（R14「插件故障不得拖垮主程序」怎么实现）。

  这些如果只写在代码里，下一个人会重新发明一套结论；而 v0.1 的权限是**故意不拦截**的，
  不写下来会被误读成「忘了做」。

- **Decision**:

  1. **第三方插件默认 out-of-process**，宿主用行分隔 JSON-RPC 2.0 over stdio 与子进程通信
     （与 MCP 同形态）。理由不是安全沙箱，而是**故障隔离与所有权清晰**：
     in-process 的插件 panic 会顺着调用栈穿过宿主线程，而 `catch_unwind` 只能挡住
     Rust unwind、挡不住 `abort`、栈溢出、陷入死循环的线程。子进程死掉是一件**可观察的事实**，
     宿主只需把插件标记为 `degraded` 并 lazy respawn。
     运行方式优先级保持计划书顺序：out-of-process > WASM > in-process（仅限可信内置），
     v0.1 只实现第一种。
  2. **Host API 是唯一的跨边界能力面**。插件进程拿不到数据库句柄、拿不到 Secret Store 句柄。
     计划书列出的 7 个受限 API（`host/log`、`fs.read_scoped`、`secret.resolve_ref`、
     `db.emit_event`、`config.read_owned`、`config.write_projected`、
     `process.spawn_declared`）就是全部能力面；v0.1 只真实实现 `host/log`，
     其余返回结构化 `-32003 not_implemented`。
  3. **v0.1 的权限是 observe-only，不是强制拒绝**（与规格 4.28 同口径）。
     每次 Host API 调用写一行 `permission_decisions`（`decision` 恒 `observed`），
     不做拦截。理由：真正的 Permission Engine 属于 Phase 9.2，尚未实现；
     在没有策略来源（策略档案、作用域、用户授权流）时做「拒绝」只会把
     合法调用误杀，而**先记录**能给出 9.2 需要的第一手数据。
     `manifest.toml` 的 `[capabilities]` 在 v0.1 是**声明元数据 + 允许集来源**
     （宿主只会调用声明过的通路），不是权限引擎。
     「不做」这件事本身是决策：再次强调，**不要**在 v0.1 的代码里补一套
     隐式拒绝逻辑，那会与 9.2 的设计争夺所有权。
  4. **进程遏制沿用 ADR-0013**：Windows 上 spawn 后**立即** `assign` 到 Job Object
     （`KILL_ON_JOB_CLOSE`），宿主退出带走全部插件进程；不接受
     「creation-time containment」之外的保证等级声称 —— 与 ADR-0013 同样如实写明
     存在 spawn → assign 的窗口，不声称 race-free。
  5. **故障契约**：插件握手失败/超时/崩溃，宿主记录错误并把它降级为 `degraded`，
     **绝不 panic 主线程**、绝不让一个插件影响其他插件的注册结果
     （扫描时单插件失败只记该插件自己的 error）。

- **Consequences**:

  - **好**：宿主不会因为一个示例插件写错 `json.dumps` 而崩；插件可以用任意语言实现
    （v0.1 示例是 Python stdlib-only），不需要与宿主共享 ABI。
  - **好**：`permission_decisions` 从第一天起就有真实数据，Phase 9.2 设计策略引擎时
    可以直接拿它做分布统计。
  - **代价**：进程边界意味着每个插件至少一个常驻进程，握手有 10s 超时的额外复杂度；
    v0.1 不做心跳，插件长挂后的失效只能靠下一次调用发现。
  - **代价（已知风险，接受）**：observe-only 期间，恶意插件在能做坏的范围内
    （比如伪造 usage 数据）宿主拦不住。缓解手段是数据面契约：
    usage 导入只走现有 `parse_report` → `normalize_session` 校验与幂等 key，
    插件不能绕过 schema 直接写库。
  - **触发复议**：Phase 9.2 Permission Engine 落地时，本 ADR 的第 3 条需要改写成
    「拒绝 + 记录」；WASM 运行方式落地时需要补充它与本 ADR 的关系。

- **Alternatives considered**:

  - **in-process trait object**（`Arc<dyn HarnessAdapter>`）：性能最好、无需序列化，
    但 panic/abort/死循环会穿透宿主，与 R14 直接冲突 → 否决。
  - **WASM 沙箱**：真正的强制隔离，但需要插件作者用特定工具链编译，
    v0.1 的目标是「冻结契约并跑通」，不是「建立沙箱」 → 推迟到 WASM 阶段。
  - **v0.1 就直接强制拒绝越权调用**：在没有策略来源时会误杀合法调用，
    且会与 9.2 争夺所有权 → 否决（改为 observe-only）。
