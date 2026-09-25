//! 集成测试共用的小工具（`tests/common/` 目录不会被 Cargo 当成独立 target）。
//!
//! 这里只放**纯函数**：真机 E2E 的「这台机器到底有没有用量数据」前提判定。
//! 之所以共用，是因为 `real_ccusage_import.rs` 与 `real_dashboard_summary.rs` 必须遵守
//! **同一条**规则；两处各写一份一定会漂移。

#![allow(dead_code)] // 每个 test binary 只用到其中一部分

use serde_json::Value;

/// 真机 E2E 的前提判定结果。
#[derive(Debug, PartialEq, Eq)]
pub enum Precondition {
    /// 来源报告**本身**是空的：这台机器确实没有用量数据 → 明确跳过。
    NoDataInSource,
    /// 报告里有行，导入却得到 0 条 → adapter 丢行，必须**硬失败**（不能被跳过掩盖）。
    SourceHadRowsButNothingImported,
    /// 正常：既有数据也导入了，继续跑完整对账。
    Proceed,
}

/// 报告本身是否为空：`session` 与 `daily` 两个 section 都没有行。
///
/// 缺字段（例如 `empty.json` 没有 `daily`）按 0 行处理。
pub fn report_is_blank(report: &Value) -> bool {
    fn rows(report: &Value, key: &str) -> usize {
        report[key].as_array().map_or(0, Vec::len)
    }
    rows(report, "session") == 0 && rows(report, "daily") == 0
}

/// 前提判定：**先看报告本身是否为空**，再看导入条数。
///
/// 顺序是关键。早期版本只看「导入 0 条就跳过」，那会把「报告里有行、adapter 却把行丢光」
/// 这种真实回归一起吞掉（跳过 = 通过，见本文件 `rows_in_the_report_but_nothing_imported…`
/// 那条测试）。所以规则是：来源空 → 跳过；来源有行但一条没导入 → **失败**。
pub fn precondition(report: &Value, records_seen: u64) -> Precondition {
    if report_is_blank(report) {
        return Precondition::NoDataInSource;
    }
    if records_seen == 0 {
        return Precondition::SourceHadRowsButNothingImported;
    }
    Precondition::Proceed
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECTIONS: &str = include_str!("../../../fixtures/ccusage/sections.json");
    const EMPTY: &str = include_str!("../../../fixtures/ccusage/empty.json");

    fn parse(raw: &str) -> Value {
        serde_json::from_str(raw).expect("fixture 必须是合法 JSON")
    }

    /// 复现缺口：报告里有行（sections.json: session 2 行 / daily 2 行）却一条都没导入时，
    /// 判定**必须**是「失败」而不是「跳过」—— 否则 adapter 丢行的真实回归会被跳过吞掉。
    #[test]
    fn rows_in_the_report_but_nothing_imported_is_a_failure_not_a_skip() {
        assert_eq!(
            precondition(&parse(SECTIONS), 0),
            Precondition::SourceHadRowsButNothingImported,
            "报告有行却导入 0 条 = adapter 丢行，绝不能判成「机器没有数据」"
        );
    }

    /// 只有报告本身为空才算「机器没有数据」。
    #[test]
    fn a_blank_report_is_the_only_no_data_shape() {
        assert_eq!(
            precondition(&parse(EMPTY), 0),
            Precondition::NoDataInSource,
            "空报告（session/daily 都没有行）才是真正的「没有数据」"
        );
    }

    /// 正常形状：有行、也导入了 → 继续跑对账。
    #[test]
    fn rows_with_an_actual_import_proceed() {
        assert_eq!(precondition(&parse(SECTIONS), 7), Precondition::Proceed);
    }
}
