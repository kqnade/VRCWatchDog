//! 「VRChat が起動中かどうか」の前回観測値と今回観測値から、遷移を判定する pure 関数。
//!
//! sysinfo を切り離してあるので、actor のループから独立にテストできる。

/// VRChat プロセス状態の遷移。actor のメインループが 1 tick 毎に算出する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessTransition {
    /// 前回 false → 今回 true。VRChat が起動した瞬間。
    Started,
    /// 前回 true → 今回 false。VRChat が終了した瞬間 (= ClosedWithoutJoin 候補)。
    Stopped,
    /// 前回と今回が同じ状態。
    NoChange,
}

/// `previous_was_running` と `currently_running` から遷移種別を返す。
///
/// actor の 1 tick 内処理:
/// ```ignore
/// let now = is_vrchat_process_running(&mut sysinfo);
/// match detect_transition(prev_state, now) {
///     ProcessTransition::Stopped => emit_to_projector(),
///     ProcessTransition::Started => log,
///     ProcessTransition::NoChange => {}
/// }
/// prev_state = now;
/// ```
pub fn detect_transition(previous_was_running: bool, currently_running: bool) -> ProcessTransition {
    match (previous_was_running, currently_running) {
        (false, true) => ProcessTransition::Started,
        (true, false) => ProcessTransition::Stopped,
        _ => ProcessTransition::NoChange,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_started_when_previous_false_current_true() {
        assert_eq!(detect_transition(false, true), ProcessTransition::Started);
    }

    #[test]
    fn detect_returns_stopped_when_previous_true_current_false() {
        assert_eq!(detect_transition(true, false), ProcessTransition::Stopped);
    }

    #[test]
    fn detect_returns_no_change_when_both_false() {
        // 起動前の安定状態 (まだ VRChat が起動していない)
        assert_eq!(detect_transition(false, false), ProcessTransition::NoChange);
    }

    #[test]
    fn detect_returns_no_change_when_both_true() {
        // VRChat 走行中の安定状態
        assert_eq!(detect_transition(true, true), ProcessTransition::NoChange);
    }
}
