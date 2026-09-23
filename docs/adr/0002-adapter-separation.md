# ADR-0002 — HarnessAdapter 与 UsageAdapter 分离

- **Status**: Accepted（2026-04）
- **Context**: "能启动某个 Harness"与"能可靠解析它的 Token / Cost"是两件独立的事。不同 Harness
  的数据可得性差异巨大（有的能拿到精确 usage，有的只能估算，有的完全没有）。若两者绑定成一个
  适配器，那么"Usage 不可得"会直接导致"该 Harness 无法被管理"。
- **Decision**: 拆成两个独立 trait：

  ```rust
  trait HarnessAdapter {          // 生命周期
      fn id(&self) -> HarnessId;
      fn detect(&self) -> DetectResult;
      fn version(&self) -> Result<Option<String>>;
      fn capabilities(&self) -> HarnessCapabilities;
      fn launch(&self, req: LaunchRequest) -> Result<ProcessHandle>;
      fn resume(&self, req: ResumeRequest) -> Result<ProcessHandle>;
  }

  trait UsageAdapter {            // 数据
      fn source(&self) -> HarnessId;
      fn discover(&self) -> Result<Vec<DataSource>>;
      fn import(&self, range: ImportRange) -> Result<ImportResult>;
      fn watch(&self) -> Result<()>;
  }
  ```

  日志解析**不得**塞进 `HarnessAdapter`。能力用 `HarnessCapabilities` 矩阵表达，而不是单个 boolean。

- **Alternatives**:
  - 单一 `HarnessAdapter` 包办一切：接口简单，但会把"Usage 可得性"耦合进启动路径。
  - 每个 Harness 一套无接口的具体实现：初期更快，但无法做 cross-harness 聚合与能力 UI。
- **Consequences**:
  - 一个没有可靠 Token 数据的 Harness 依然可以被 Harness Hub 完整管理（只标记 usage 能力为不支持）。
  - UI 可以按能力逐项灰度（launch ✓ / usage ✗ / replay ? ）。
  - v0.1 的 Usage 优先通过 ccusage JSON 接口归一化，不逐个 Harness 重写 parser。
- **Evidence**: `src-tauri/src/harness/adapter.rs`、`src-tauri/src/usage/adapter.rs`，
  以及 `HarnessRegistry` 的注册/查询测试。
- **Revisit Conditions**: 若某个 Harness 的生命周期与数据在协议层面完全不可分（例如自带结构化
  事件流且无外部文件），再评估合并。
