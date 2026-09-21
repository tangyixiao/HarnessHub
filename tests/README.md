# tests

跨模块测试。

```text
fixtures/     集成测试共用的夹具
integration/  Rust / 前端 / sidecar 之间的集成测试
e2e/          端到端主流程（启动应用 → 启动 Harness → 导入 Usage → 查看 Dashboard）
```

单元测试与实现放在一起（Rust 的 `#[cfg(test)] mod tests`、前端的 `*.test.ts(x)`）。
本目录只放**跨进程 / 跨模块**的测试。
