# schemas

Harness Hub 对外可移植的数据格式 schema 草案。

```text
hhar/             Harness Hub Activity Record（长期活动数据导出，ADR-0019）
events/           Canonical Event（Trace 平面的统一事件）
plugin-manifest/  插件清单
```

## 当前状态：**刻意未冻结**

规格风险 R16 明确警告「开放事件格式过早冻结」。因此在满足以下条件之前，
这里不会出现 `*.schema.json`：

1. 有至少两个真实数据源（≥2 个 Harness）能映射到同一事件结构；
2. 有对应的 fixture 与 contract test；
3. 有 ADR 记录该格式的取舍与兼容策略（对齐 OTel / OpenInference，ADR-0017）。

在此之前，事件结构由 Rust 侧领域类型定义（`src-tauri/src/trace/`），
schema 目录只记录规划，不发布不稳定格式。
