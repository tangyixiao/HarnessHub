//! `stable_source_key`：幂等去重的唯一来源。
//!
//! ADR-0011 决策三。要点：
//!
//! * 键 = `sha256(canonical_payload)`，`canonical_payload` **长度前缀**编码每一个维度，
//!   因此 `period = "a:b"` 与 `model = "c"` 不可能和 `period = "a"` 与 `model = "b:c"`
//!   撞成同一个键（朴素字符串拼接会）。
//! * payload 里带 `key_version`。将来 ccusage 增加身份维度时，**升版本**而不是改算法，
//!   否则历史行的键会静默漂移，去重与对账同时失效。

use sha2::{Digest, Sha256};

/// 当前使用的键版本。升级它是一次显式的、需要 ADR 的决定。
pub const KEY_VERSION: i64 = 1;

/// 构成一个 UsageEvent 身份的全部维度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyDimensions<'a> {
    /// 数据来源，例如 `"ccusage"`。
    pub source: &'a str,
    /// 报告类型，例如 `"session"`（`daily` 永不导入，但维度必须显式带上）。
    pub report_kind: &'a str,
    /// 外部 Harness 名（ccusage 的 `agent`）。
    pub harness: &'a str,
    /// 外部会话身份（ccusage 的 `period`）。
    pub source_session_id: &'a str,
    /// 模型名（ccusage 的 `modelName`）。
    pub model: &'a str,
}

/// 未加哈希的规范化字节串。
///
/// 单独暴露是为了让测试能直接断言「编码本身无歧义」，而不是只看哈希结果。
pub fn canonical_payload(version: i64, dimensions: &KeyDimensions<'_>) -> Vec<u8> {
    let mut payload = Vec::with_capacity(96);
    payload.extend_from_slice(format!("usage-event-v{version}").as_bytes());
    for field in [
        dimensions.source,
        dimensions.report_kind,
        dimensions.harness,
        dimensions.source_session_id,
        dimensions.model,
    ] {
        // 长度前缀：读完长度就知道字段到哪里结束，分隔符因此不可能产生歧义。
        payload.push(0);
        payload.extend_from_slice(field.len().to_string().as_bytes());
        payload.push(b':');
        payload.extend_from_slice(field.as_bytes());
    }
    payload
}

/// 指定版本的键（测试用它证明「升版本会改变键」）。
pub fn key_for(version: i64, dimensions: &KeyDimensions<'_>) -> String {
    let digest = Sha256::digest(canonical_payload(version, dimensions));
    hex::encode(digest)
}

/// 生产路径：用当前 `KEY_VERSION` 生成键。
pub fn stable_source_key(dimensions: &KeyDimensions<'_>) -> String {
    key_for(KEY_VERSION, dimensions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dims<'a>(harness: &'a str, period: &'a str, model: &'a str) -> KeyDimensions<'a> {
        KeyDimensions {
            source: "ccusage",
            report_kind: "session",
            harness,
            source_session_id: period,
            model,
        }
    }

    #[test]
    fn key_is_stable_for_the_same_dimensions() {
        let first = stable_source_key(&dims("codex", "rollout-1", "gpt-5.6-sol"));
        let second = stable_source_key(&dims("codex", "rollout-1", "gpt-5.6-sol"));

        assert_eq!(first, second, "同一份源数据必须永远产生同一个键");
        assert_ne!(
            first,
            stable_source_key(&dims("codex", "rollout-1", "gpt-5.6-terra")),
            "不同模型必须是不同的键"
        );
        assert_ne!(
            first,
            stable_source_key(&dims("claude", "rollout-1", "gpt-5.6-sol")),
            "不同 harness 必须是不同的键"
        );
    }

    #[test]
    fn key_is_a_lowercase_hex_sha256() {
        let key = stable_source_key(&dims("codex", "rollout-1", "gpt-5.6-sol"));

        assert_eq!(key.len(), 64, "sha256 十六进制应为 64 字符：{key}");
        assert!(
            key.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "必须是规范化的小写十六进制：{key}"
        );
    }

    #[test]
    fn key_separates_models_inside_one_session() {
        // 实测：20 个 session 行含多个 modelBreakdowns。一行一个模型，键必须分开。
        let sol = stable_source_key(&dims("codex", "rollout-multi", "gpt-5.6-sol"));
        let terra = stable_source_key(&dims("codex", "rollout-multi", "gpt-5.6-terra"));

        assert_ne!(sol, terra);
    }

    /// 朴素拼接（`source:kind:agent:period:model`）在这里会产生伪碰撞，长度前缀不会。
    #[test]
    fn key_has_no_separator_ambiguity() {
        let left = stable_source_key(&dims("codex", "a:b", "c"));
        let right = stable_source_key(&dims("codex", "a", "b:c"));

        assert_ne!(
            left, right,
            "字段边界必须由编码决定，而不是由内容里恰好出现的分隔符决定"
        );
    }

    /// 编码本身是契约：任何维度改名/增删/换格式都必须让这条断言失败。
    ///
    /// 用**精确**字符串而不是 `contains`，否则「少了一个维度」也能通过。
    #[test]
    fn canonical_payload_is_an_exact_length_prefixed_encoding() {
        let payload = canonical_payload(KEY_VERSION, &dims("codex", "rollout-1", "gpt-5.6-sol"));
        let text = String::from_utf8(payload).expect("payload 是 ASCII");

        assert_eq!(
            text,
            "usage-event-v1\u{0}7:ccusage\u{0}7:session\u{0}5:codex\u{0}9:rollout-1\u{0}11:gpt-5.6-sol"
        );
    }

    #[test]
    fn key_changes_when_the_version_changes() {
        let dimensions = dims("codex", "rollout-1", "gpt-5.6-sol");

        assert_ne!(
            key_for(1, &dimensions),
            key_for(2, &dimensions),
            "升级键版本必须改变键，否则版本号是装饰"
        );
        assert_eq!(
            stable_source_key(&dimensions),
            key_for(KEY_VERSION, &dimensions),
            "生产路径必须用当前版本"
        );
    }

    /// 用真实夹具证明：222 行 × 全部 breakdown 上没有 natural-key 碰撞。
    ///
    /// 这里刻意用 `serde_json::Value` 而不是解析模型 —— 测的是键，不是解析器。
    #[test]
    fn key_has_no_collisions_in_the_committed_fixture() {
        let raw = include_str!("../../../fixtures/ccusage/session.json");
        let report: serde_json::Value = serde_json::from_str(raw).expect("夹具必须可解析");
        let sessions = report["session"].as_array().expect("session 数组");

        let mut keys = std::collections::BTreeSet::new();
        let mut breakdowns = 0usize;
        for row in sessions {
            let harness = row["agent"].as_str().expect("agent");
            let period = row["period"].as_str().expect("period");
            for breakdown in row["modelBreakdowns"].as_array().expect("modelBreakdowns") {
                let model = breakdown["modelName"].as_str().expect("modelName");
                keys.insert(stable_source_key(&dims(harness, period, model)));
                breakdowns += 1;
            }
        }

        assert!(breakdowns > 0, "夹具必须真的含 breakdown");
        assert_eq!(
            keys.len(),
            breakdowns,
            "{breakdowns} 个 breakdown 只产生了 {} 个不同的键",
            keys.len()
        );
    }
}
