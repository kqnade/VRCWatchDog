//! Tauri ↔ frontend の境界契約。
//!
//! - [`events`] はバックエンドから frontend に emit する全イベント型。
//! - [`commands`] は frontend → backend のコマンドハンドラ (Tauri 非依存の純関数)。
//!
//! Tauri 層 (`crates/app`) では `#[tauri::command]` で薄くラップするだけで、
//! ロジックは全て本モジュールにある。これにより
//! - Linux/WSL でテスト実行可能
//! - Tauri を介さない再利用 (CLI / 将来の HTTP layer) が容易
//! - フロント TS と型同期しやすい (serde rename + ts-rs 候補)
//!
//! 全 payload は `serde::{Serialize, Deserialize}` 実装、`#[serde(rename_all = "camelCase")]`
//! で TS 側に自然な命名で渡る。

pub mod commands;
pub mod events;
