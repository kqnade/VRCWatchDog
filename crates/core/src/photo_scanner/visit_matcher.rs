//! 写真の `taken_utc` に対応する `world_visits` 行を二分探索で見つける。
//!
//! C# 版 (`VRCTimeline/Helpers/WorldVisitMatcher.cs`) の Rust 移植 (plan §3)。
//! photo_scanner actor が batch ごとに `load_world_visit_ranges` で sort 済み Vec を
//! 取得し、各 photo に対し [`match_photo_to_visit`] で 1 件ずつ引く想定。
//!
//! 不変条件: 入力 `visits` は **`joined_utc` ASC で sort 済み** であること。
//! 順序が崩れていると binary search が誤った visit を返す。本モジュールでは sort
//! しない (DB から `ORDER BY joined_utc ASC` で取る前提)。
//!
//! interval は **半開区間 `[joined_utc, left_utc)`** で扱う。`left_utc` が等しい瞬間に
//! 撮影された写真は「離室直後」とみなして次の visit (もしあれば) には属さない。
//! `left_utc` が `None` の visit は「現在進行中」とみなし、joined_utc 以降の任意の
//! 写真にマッチする (上限なし)。

use chrono::{DateTime, Utc};

/// matcher の入力。`world_visits` row のうち時刻だけ抜き出した shape。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldVisitTimeRange {
    pub id: i64,
    pub joined_utc: DateTime<Utc>,
    /// `None` = 現在進行中 (まだ離室していない / `left_utc` 未確定)。
    pub left_utc: Option<DateTime<Utc>>,
}

/// `photo_taken_utc` を含む visit を返す。該当無しなら `None`。
///
/// アルゴリズム:
/// 1. `partition_point(|v| v.joined_utc <= photo_taken_utc)` で「joined が photo
///    より後の最初の visit」のインデックスを得る。
/// 2. その手前 (= `idx - 1`) が候補。`idx == 0` なら photo はどの visit よりも前。
/// 3. 候補の `left_utc` が `Some(left)` かつ `left <= photo_taken_utc` なら、
///    photo はその visit の終了後 (まだ次が始まっていない隙間) なので `None`。
/// 4. それ以外 (`left_utc` が None or `photo < left`) は候補の id を返す。
///
/// 計算量 O(log n)。
pub fn match_photo_to_visit(
    visits_sorted_by_joined_utc: &[WorldVisitTimeRange],
    photo_taken_utc: DateTime<Utc>,
) -> Option<i64> {
    // partition_point: pred が true で続く範囲の長さを返す。
    // joined_utc <= taken なら true、つまり「photo 時点 or それより早く始まった visit」を
    // 全部数えた数 = 候補 visit の (1-based) 個数。
    let candidate_count =
        visits_sorted_by_joined_utc.partition_point(|v| v.joined_utc <= photo_taken_utc);
    if candidate_count == 0 {
        return None;
    }
    let candidate = &visits_sorted_by_joined_utc[candidate_count - 1];
    match candidate.left_utc {
        // 半開区間: left_utc <= taken は「ちょうど離室時刻 or それ以降」 = 範囲外
        Some(left) if left <= photo_taken_utc => None,
        _ => Some(candidate.id),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, TimeZone};

    fn utc(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.from_utc_datetime(
            &NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(h, mi, s)
                .unwrap(),
        )
    }

    /// helper: `[joined, left)` interval の visit を id 付きで作る。
    fn closed(id: i64, joined: DateTime<Utc>, left: DateTime<Utc>) -> WorldVisitTimeRange {
        WorldVisitTimeRange {
            id,
            joined_utc: joined,
            left_utc: Some(left),
        }
    }

    /// helper: `left_utc` が None (現在進行中) の visit。
    fn ongoing(id: i64, joined: DateTime<Utc>) -> WorldVisitTimeRange {
        WorldVisitTimeRange {
            id,
            joined_utc: joined,
            left_utc: None,
        }
    }

    // -- 単純ケース ----------------------------------------------------------------

    #[test]
    fn returns_none_when_visits_is_empty() {
        let got = match_photo_to_visit(&[], utc(2026, 5, 10, 12, 0, 0));

        assert!(got.is_none());
    }

    #[test]
    fn returns_none_when_photo_taken_before_any_visit_started() {
        let visits = [closed(
            1,
            utc(2026, 5, 10, 13, 0, 0),
            utc(2026, 5, 10, 14, 0, 0),
        )];

        let got = match_photo_to_visit(&visits, utc(2026, 5, 10, 12, 0, 0));

        assert!(got.is_none());
    }

    #[test]
    fn returns_visit_id_when_photo_falls_inside_visit_interval() {
        let visits = [closed(
            42,
            utc(2026, 5, 10, 12, 0, 0),
            utc(2026, 5, 10, 13, 0, 0),
        )];

        let got = match_photo_to_visit(&visits, utc(2026, 5, 10, 12, 30, 0));

        assert_eq!(got, Some(42));
    }

    // -- 境界条件: 半開区間 [joined, left) ---------------------------------------

    #[test]
    fn matches_photo_taken_at_exactly_joined_utc_boundary() {
        // joined 時刻ぴったりは visit に含まれる (closed lower bound)
        let joined = utc(2026, 5, 10, 12, 0, 0);
        let visits = [closed(7, joined, utc(2026, 5, 10, 13, 0, 0))];

        let got = match_photo_to_visit(&visits, joined);

        assert_eq!(got, Some(7));
    }

    #[test]
    fn does_not_match_photo_taken_at_exactly_left_utc_boundary() {
        // left 時刻ぴったりは visit に含まれない (open upper bound)
        let left = utc(2026, 5, 10, 13, 0, 0);
        let visits = [closed(7, utc(2026, 5, 10, 12, 0, 0), left)];

        let got = match_photo_to_visit(&visits, left);

        assert!(
            got.is_none(),
            "半開区間 [joined, left) で left 自体は範囲外"
        );
    }

    // -- 隙間 (visit 間) -----------------------------------------------------------

    #[test]
    fn returns_none_when_photo_falls_between_two_consecutive_visits() {
        // visit1: 12:00-12:30, visit2: 13:00-13:30、写真は 12:45 の隙間
        let visits = [
            closed(1, utc(2026, 5, 10, 12, 0, 0), utc(2026, 5, 10, 12, 30, 0)),
            closed(2, utc(2026, 5, 10, 13, 0, 0), utc(2026, 5, 10, 13, 30, 0)),
        ];

        let got = match_photo_to_visit(&visits, utc(2026, 5, 10, 12, 45, 0));

        assert!(
            got.is_none(),
            "visit1 の left 後 / visit2 の joined 前は隙間"
        );
    }

    // -- 進行中 (left_utc = None) -------------------------------------------------

    #[test]
    fn matches_photo_within_ongoing_visit_with_no_left_utc() {
        // 1 件だけの ongoing visit、写真は join 後の任意時刻
        let visits = [ongoing(99, utc(2026, 5, 10, 12, 0, 0))];

        let got = match_photo_to_visit(&visits, utc(2026, 5, 10, 23, 0, 0));

        assert_eq!(
            got,
            Some(99),
            "left_utc=None の visit は join 以降の任意 photo にマッチ"
        );
    }

    #[test]
    fn returns_none_for_photo_taken_before_ongoing_visit_started() {
        let visits = [ongoing(99, utc(2026, 5, 10, 12, 0, 0))];

        let got = match_photo_to_visit(&visits, utc(2026, 5, 10, 11, 59, 59));

        assert!(got.is_none());
    }

    // -- 多 visit chain での選択 -------------------------------------------------

    #[test]
    fn picks_correct_visit_from_a_chain_of_three() {
        // visit1 12-13, visit2 14-15, visit3 16-17
        // 写真は 14:30 = visit2 の中
        let visits = [
            closed(1, utc(2026, 5, 10, 12, 0, 0), utc(2026, 5, 10, 13, 0, 0)),
            closed(2, utc(2026, 5, 10, 14, 0, 0), utc(2026, 5, 10, 15, 0, 0)),
            closed(3, utc(2026, 5, 10, 16, 0, 0), utc(2026, 5, 10, 17, 0, 0)),
        ];

        let got = match_photo_to_visit(&visits, utc(2026, 5, 10, 14, 30, 0));

        assert_eq!(got, Some(2));
    }

    #[test]
    fn picks_latest_ongoing_visit_when_multiple_visits_share_join_le_photo() {
        // visit1 (closed) 10-11, visit2 (ongoing) 12 〜 、写真 13:00
        // visit2 がアクティブなのでそちら
        let visits = [
            closed(1, utc(2026, 5, 10, 10, 0, 0), utc(2026, 5, 10, 11, 0, 0)),
            ongoing(2, utc(2026, 5, 10, 12, 0, 0)),
        ];

        let got = match_photo_to_visit(&visits, utc(2026, 5, 10, 13, 0, 0));

        assert_eq!(got, Some(2));
    }

    // -- ストレス: 1000 visit ----------------------------------------------------
    //
    // 二分探索が線形 fallback していないことを大雑把に確認する性能スモーク。
    // wallclock の閾値を assert すると CI で flaky になりがちなので、結果の正しさだけ assert。

    #[test]
    fn handles_thousand_visits_and_returns_correct_match() {
        // visit i: joined=2026-01-01 + i 分、left=joined + 30 秒 (= 30 秒間アクティブ)
        let mut visits = Vec::with_capacity(1000);
        let base = utc(2026, 1, 1, 0, 0, 0);
        for i in 0..1000i64 {
            let joined = base + chrono::Duration::minutes(i);
            let left = joined + chrono::Duration::seconds(30);
            visits.push(WorldVisitTimeRange {
                id: i + 1,
                joined_utc: joined,
                left_utc: Some(left),
            });
        }

        // visit 500 (joined = base + 500 min) のちょうど 15 秒後の photo
        let photo = base + chrono::Duration::minutes(500) + chrono::Duration::seconds(15);

        let got = match_photo_to_visit(&visits, photo);

        assert_eq!(got, Some(501), "1-based id なので 500 + 1 = 501");
    }
}
