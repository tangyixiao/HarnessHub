---
name: ipc-contract-test
description: 为 Tauri 命令写一条锁死 JSON 键名的契约测试，防止 camelCase / snake_case 漂移。
---

# 为 Rust 命令写 IPC 契约测试

Tauri 的 DTO 最容易出的事故不是逻辑错，而是**键名悄悄变了**：前端读 `totalTokens`
而 Rust 序列化成 `total_tokens`，TS 侧拿到 `undefined` 却不报错。本技能给出最短可行的
防护流程。

## 1. 找到真正的跨边界类型

跨 Tauri IPC 的类型只包括：`#[tauri::command]` 的参数与返回值、`emit` 出去的事件载荷。
内部 struct 不需要契约测试。

```rust
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSession {
    pub session_id: String,
    pub total_tokens: u64,
}
```

记住：`rename_all` 只改**结构体字段**；enum 变体里的字段要另外加
`rename_all_fields`，否则变体内部仍是 snake_case。

## 2. 先写测试，让它先红

```rust
#[cfg(test)]
mod contract_tests {
    use super::*;

    #[test]
    fn usage_session_wire_shape_is_locked() {
        let value = serde_json::to_value(UsageSession {
            session_id: "sess_1".to_string(),
            total_tokens: 42,
        })
        .unwrap();

        // 断言键名，不要断言整个 JSON 字符串（顺序与空白会变）。
        assert_eq!(value["sessionId"], serde_json::json!("sess_1"));
        assert_eq!(value["totalTokens"], serde_json::json!(42));
        assert!(
            value.get("session_id").is_none(),
            "snake_case 键泄漏到线上了：{value}"
        );
    }
}
```

先跑一次确认它变红（把 `rename_all` 注释掉再跑），**看到失败**才算这条测试有效。

## 3. 在 TS 侧配一份同形状 fixture

Rust 契约测试只能证明「我们发出的是这个形状」，还需要一条用同样 payload 锁住
解析行为与安全默认值的测试：

```ts
const payload = { sessionId: 'sess_1', totalTokens: 42 };
const parsed = normalizeUsageSession(payload);
expect(parsed.sessionId).toBe('sess_1');
expect(normalizeUsageSession({ session_id: 'sess_1' }).sessionId).toBeUndefined();
```

第二条断言故意喂链路上不该出现的 snake_case：它应该被忽略，而不是被宽容地接住。

## 4. 检查清单

- [ ] 每条跨 IPC 的 DTO 都有自己的 contract test，断言**键名**。
- [ ] enum 变体字段用 `rename_all_fields`，并有测试覆盖。
- [ ] 可选字段的 `null` / 缺省行为被测试钉死（`skip_serializing_if` 会改变形状）。
- [ ] TS 侧有对应 fixture test，且包含一份「错误形状应被拒绝」的用例。
- [ ] 测试断言的是 `serde_json::Value`，不是格式化后的字符串。
