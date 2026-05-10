//! sysinfo ベースの実 VRChat プロセス監視 actor。
//!
//! `System::refresh_processes_specifics` をループで叩き、
//! [`detect_transition`](super::detect::detect_transition) で遷移種別を計算する。
//!
//! Phase 7.4.1 では遷移を tracing log に出すだけ。projector への通知 (= 実 finalization)
//! は 7.4.2 で channel 経由で接続する。

use std::time::Duration;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

use super::detect::{detect_transition, ProcessTransition};

/// VRChat の実行ファイル名 (Windows 標準)。
pub const VRCHAT_PROCESS_NAME: &str = "VRChat.exe";

/// VRChatProcessMonitor の設定。
#[derive(Debug, Clone)]
pub struct VRChatProcessMonitorConfig {
    /// プロセス再走査の間隔。秒オーダーで検出できれば十分。
    pub poll_interval: Duration,
    /// マッチさせるプロセス名。tests からは fake 名を入れてコールバック経路を確認できる。
    pub process_name: String,
}

impl Default for VRChatProcessMonitorConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(2),
            process_name: VRCHAT_PROCESS_NAME.to_string(),
        }
    }
}

/// `System` 上で「指定プロセス名が 1 つでも走っていれば true」を返す。
///
/// sysinfo の API は OS によって case sensitivity が違うが、Windows の `VRChat.exe` は
/// 完全一致で十分。クロスプラットフォーム化が必要になったら case-insensitive 化する。
pub fn is_vrchat_process_running(system: &System, process_name: &str) -> bool {
    system
        .processes()
        .values()
        .any(|p| p.name().to_string_lossy() == process_name)
}

/// VRChat プロセスを監視する actor。
///
/// `run()` は無限ループ。stop は外部 (JoinSet::abort 等) で行う。
pub struct VRChatProcessMonitor {
    config: VRChatProcessMonitorConfig,
}

impl VRChatProcessMonitor {
    pub fn new(config: VRChatProcessMonitorConfig) -> Self {
        Self { config }
    }

    /// メインループ。`poll_interval` ごとに sysinfo を refresh し、`detect_transition`
    /// で遷移種別を計算、Started/Stopped を log する。
    pub async fn run(self) {
        // sysinfo は重いので 1 度だけ作って refresh で使い回す。
        // ProcessRefreshKind::new() = CPU/mem を読まず name のみ収集。
        let mut system = System::new();

        // 初回 tick は「観測した現状を初期化値とする」方針: 起動時に VRChat が既に
        // 動いていても Started を発火しない (= 起動直後の偽の遷移を防ぐ)。
        system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::new());
        let mut prev_running = is_vrchat_process_running(&system, &self.config.process_name);
        if prev_running {
            tracing::info!(
                process = %self.config.process_name,
                "process_monitor: target already running at startup",
            );
        } else {
            tracing::info!(
                process = %self.config.process_name,
                "process_monitor: target not running at startup",
            );
        }

        loop {
            tokio::time::sleep(self.config.poll_interval).await;

            system.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::new(),
            );
            let now_running = is_vrchat_process_running(&system, &self.config.process_name);

            match detect_transition(prev_running, now_running) {
                ProcessTransition::Started => {
                    tracing::info!(process = %self.config.process_name, "VRChat started");
                }
                ProcessTransition::Stopped => {
                    // Phase 7.4.2 で projector finalize を呼ぶ予定の hook ポイント。
                    // 現時点では log のみ (ClosedWithoutJoin 遷移はまだ起きない)。
                    tracing::info!(
                        process = %self.config.process_name,
                        "VRChat stopped (ClosedWithoutJoin finalization pending in 7.4.2)",
                    );
                }
                ProcessTransition::NoChange => {}
            }
            prev_running = now_running;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn is_vrchat_process_running_returns_false_when_no_process_matches() {
        // Arrange: 何も走っていないと判定したい場合は、絶対にマッチしない名前を渡す
        let mut system = System::new();
        system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::new());

        // Act
        let running = is_vrchat_process_running(
            &system,
            "this_should_definitely_not_be_running_xyz_12345.exe",
        );

        // Assert
        assert!(!running);
    }

    #[test]
    fn is_vrchat_process_running_can_detect_at_least_one_known_process() {
        // テスト実行中の自プロセスは必ず存在する。Cargo の test runner の名前は
        // ランナーによって異なるので「何でもいいので 1 つでも見つかる前提で
        // process が空ではない」ことだけ確認する。
        let mut system = System::new();
        system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::new());
        assert!(
            !system.processes().is_empty(),
            "refresh_processes_specifics で 1 件以上のプロセスは見えるはず"
        );
    }

    #[test]
    fn config_default_uses_two_second_interval_and_vrchat_exe() {
        let cfg = VRChatProcessMonitorConfig::default();
        assert_eq!(cfg.poll_interval, Duration::from_secs(2));
        assert_eq!(cfg.process_name, "VRChat.exe");
    }
}
