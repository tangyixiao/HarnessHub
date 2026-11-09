# F23 执行计划（HF 模型资产 + 打包态 Python 资源定位）

> 规格：`docs/specs/2026-11-09-f23-model-assets-design.md`（契约唯一事实来源）。

## 切片

1. **S1 `feat/f23/py-model-assets`**：`models/` 模块（status/info/cache/download）
   + `rpc/server.py` 方法表扩展 + `python/tests/test_models_*.py`。
   前置：F22 S1 已合入（server.py 冲突面）。
2. **S2 `feat/f23/rust`**：`sidecar/models.rs` DTO + fail-closed decoder + 6 个薄命令
   + serde 契约测试 + `sidecar/config.rs` 打包态资源定位（S4 并入）+ 单测。
3. **S3 `feat/f23/web`**：`ipc.ts` normalize + `ModelAssetsCard` + fixture tests。

合并顺序 S1 → S2 → S3。PR 附证据表。

## 验收对照（§8.5 + CONTEXT 遗留）

- [ ] HF repo metadata（S1 models.info REST + S2 命令 + S3 UI）
- [ ] Cache discovery（S1 cache.list 扫描 + S3 只读清单）
- [ ] Explicit download（S1 download 状态机 + S3 confirm + 磁盘警示）
- [ ] Revision pinning（revision 必填参数贯穿 info/download/cache）
- [ ] Transformers experimental runtime（如实降级：永不 import，status 探测如实 false）
- [ ] Local runtime capability API（models.status SDK 探测）
- [ ] 打包态 Python 资源定位与 PYTHONPATH 注入（S2 config.rs 探测链 + 单测）
