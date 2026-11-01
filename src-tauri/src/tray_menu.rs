//! System Tray 菜单模型（ROADMAP E19，规划 §4.25）。
//!
//! **纯模块**：菜单项定义 / 文案 / 禁用态 / 动作枚举不依赖任何 Tauri GUI 类型
//! （ADR-0012 同款纪律——可测逻辑放没有 GUI 依赖的模块里，headless CI 验证到
//! 菜单模型为止；图标显示 / 菜单弹出 / 聚焦 / 退出是真机验收项）。
//!
//! v0.4 边界（spec Non-goals）：不做 Profile 切换（`ProfileAdapter.activate`
//! 尚不存在，切换必须走同一 ProfileAdapter + audit log）；不做 close-to-tray。

use serde::{Deserialize, Serialize};

/// 菜单项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayMenuItem {
    /// 菜单文本。
    pub label: String,
    /// 禁用态（Active Sessions 是只读计数，禁用）。
    pub disabled: bool,
    /// 动作（disabled 项为 None）。
    pub action: Option<TrayAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayAction {
    /// 显示 + 聚焦主窗口。
    OpenDashboard,
    /// 退出应用。
    Quit,
}

/// 构建托盘菜单（构建即现查 active sessions，不做后台轮询）。
pub fn build_menu(active_sessions: u32) -> Vec<TrayMenuItem> {
    vec![
        TrayMenuItem {
            label: "打开 Harness Hub".to_string(),
            disabled: false,
            action: Some(TrayAction::OpenDashboard),
        },
        TrayMenuItem {
            label: if active_sessions == 0 {
                "没有活动会话".to_string()
            } else {
                format!("{active_sessions} 个活动会话")
            },
            disabled: true,
            action: None,
        },
        TrayMenuItem {
            label: "退出".to_string(),
            disabled: false,
            action: Some(TrayAction::Quit),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三项结构：Open Dashboard / 只读计数（禁用）/ Quit。
    #[test]
    fn menu_has_three_items_with_expected_actions() {
        let menu = build_menu(2);
        assert_eq!(menu.len(), 3);
        assert_eq!(menu[0].action, Some(TrayAction::OpenDashboard));
        assert!(!menu[0].disabled);
        assert_eq!(menu[1].action, None, "计数项只读");
        assert!(menu[1].disabled);
        assert_eq!(menu[2].action, Some(TrayAction::Quit));
    }

    /// 计数文案：0 = 没有活动会话；N = N 个活动会话。
    #[test]
    fn session_count_drives_label() {
        assert_eq!(build_menu(0)[1].label, "没有活动会话");
        assert_eq!(build_menu(1)[1].label, "1 个活动会话");
        assert_eq!(build_menu(7)[1].label, "7 个活动会话");
    }

    /// 契约（AGENTS.md）：action 的 snake_case 序列化。
    #[test]
    fn tray_menu_serializes_with_stable_keys() {
        let json = serde_json::to_value(build_menu(1)).unwrap();
        assert_eq!(json[0]["label"], "打开 Harness Hub");
        assert_eq!(json[0]["action"], "open_dashboard");
        assert_eq!(json[2]["action"], "quit");
    }
}
