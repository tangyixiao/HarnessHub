# plugins

Harness Hub 插件 SDK 与示例插件。

```text
sdk/      受限 Host API 的类型定义与文档
examples/ 示例插件
```

边界（ADR-0018）：

- 插件**不得**直接访问主 SQLite 与 Secret Store，只能通过受限 Host API。
- 插件故障不得拖垮主程序（R14）。

当前为空：v0.1 只冻结最小 Plugin API 草案与一个示例插件（Phase 9.3）。
