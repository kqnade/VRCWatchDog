//! VRChat プロセスを監視して起動/終了の遷移を検出する actor。
//!
//! plan §1 + §2:
//! > VRChat process が落ちたとき、`vrchat_proc` actor が `process_exit` event を発火
//! > するが、log に未処理行が残っているかもしれない。よって即 `ClosedWithoutJoin`
//! > 遷移はせず、**「log catch-up 完了 (該当 log file の reconcile + projection が
//! > drain)」後の finalization event** として扱う。
//!
//! 本 commit (Phase 7.4.1) は **検出 + log まで**。projector との連携 (Stopped を
//! 受けて active visit を ClosedWithoutJoin に finalize する) は 7.4.2 で別 commit。
//!
//! 設計選択:
//! - sysinfo の `refresh_processes_specifics` は CPU/mem 系の情報を含めると重いので
//!   `ProcessRefreshKind::nothing()` で「存在確認だけ」に絞る。
//! - 2 秒間隔の polling。VRChat 起動/終了は秒オーダーで検出できれば十分で、
//!   それ以下は sysinfo 自体のオーバーヘッドが目立ってくる。
//! - 検出ロジック (`detect_transition`) は pure 関数として切り出してテスト可能に。

pub mod detect;
pub mod monitor;

pub use detect::{detect_transition, ProcessTransition};
pub use monitor::{
    is_vrchat_process_running, VRChatProcessMonitor, VRChatProcessMonitorConfig,
    VRCHAT_PROCESS_NAME,
};
