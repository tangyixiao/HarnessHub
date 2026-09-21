//! 权限 / 策略引擎。
//!
//! 职责：定义权限命名空间、策略档案与作用域，把每一次许可决定记录成
//! `permission_decisions`（权限是一等数据，不是 UI 开关 —— ADR-0020）。
//!
//! v0.1 边界（规格 4.28）：只定义 schema 并记录 observed permission events，
//! 不做强制沙箱。当前为占位模块，尚未提供公开 API。
