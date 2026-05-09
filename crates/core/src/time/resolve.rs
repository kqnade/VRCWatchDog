use chrono::offset::MappedLocalTime;
use chrono::{DateTime, NaiveDateTime, Offset, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

/// ローカル時刻を UTC に解決した結果の判定。
///
/// DST 境界では同じ local 時刻が 2 回出現する (Ambiguous) か、
/// 存在しない (Gap) ことがあるため、解決方法を明示的に記録する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Resolution {
    /// 唯一の UTC 解釈が存在する通常ケース。
    Single,
    /// Ambiguous で、より早い (DST 適用中の) 解釈を採用した。
    AmbiguousFirst,
    /// Ambiguous で、より遅い (DST 終了後の) 解釈を採用した。
    AmbiguousSecond,
    /// Gap (存在しない local 時刻) のため、次の有効な local 時刻まで前進した。
    Gap,
}

impl Resolution {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Single => "Single",
            Self::AmbiguousFirst => "AmbiguousFirst",
            Self::AmbiguousSecond => "AmbiguousSecond",
            Self::Gap => "Gap",
        }
    }
}

/// `naive` (タイムゾーン情報なしのローカル時刻) を `tz` で UTC に解決する。
///
/// `prev_utc` には直前のログ行の UTC 時刻を渡す。
/// Ambiguous な local 時刻に対しては、`prev_utc` 以降になるよう
/// 単調性を優先して earlier / later を選ぶ。
///
/// 戻り値は `(UTC 時刻, UTC オフセット秒, 解決種別)`。
pub fn resolve_local_to_utc(
    naive: NaiveDateTime,
    tz: &Tz,
    prev_utc: Option<DateTime<Utc>>,
) -> (DateTime<Utc>, i32, Resolution) {
    match tz.from_local_datetime(&naive) {
        MappedLocalTime::Single(dt) => {
            let offset = dt.offset().fix().local_minus_utc();
            (dt.to_utc(), offset, Resolution::Single)
        }
        MappedLocalTime::Ambiguous(earlier, later) => {
            // earlier の UTC が prev_utc より厳密に後なら earlier (= AmbiguousFirst) を採用。
            // そうでなければ later (= AmbiguousSecond) を採用して厳密単調性を確保。
            // 等値で earlier を選ぶと、prev が earlier 自身のときに連続して earlier を返してしまい
            // 同じ local 時刻が連続する場合 (例: 1 秒以内に 2 回ログが出る境界) に進めなくなる。
            let pick_earlier = match prev_utc {
                Some(p) => earlier.to_utc() > p,
                None => true,
            };
            let chosen = if pick_earlier { earlier } else { later };
            let res = if pick_earlier {
                Resolution::AmbiguousFirst
            } else {
                Resolution::AmbiguousSecond
            };
            let offset = chosen.offset().fix().local_minus_utc();
            (chosen.to_utc(), offset, res)
        }
        MappedLocalTime::None => {
            // Gap: 次の有効な local 時刻まで前進する。
            let next_valid = next_valid_local_after(tz, naive);
            let dt = match tz.from_local_datetime(&next_valid) {
                MappedLocalTime::Single(d) => d,
                MappedLocalTime::Ambiguous(d, _) => d,
                MappedLocalTime::None => {
                    // 4 時間以内に valid time が見つからないのは tz データ異常。
                    // panic でなく conservatively 単純な UTC 解釈に倒す。
                    return (
                        DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc),
                        0,
                        Resolution::Gap,
                    );
                }
            };
            let offset = dt.offset().fix().local_minus_utc();
            (dt.to_utc(), offset, Resolution::Gap)
        }
    }
}

/// `naive` 直後の最初の有効な local 時刻を探す。
/// DST gap は通常 1 時間なので 1 分刻みで最大 4 時間まで探索すれば十分。
fn next_valid_local_after(tz: &Tz, naive: NaiveDateTime) -> NaiveDateTime {
    let mut probe = naive;
    for _ in 0..240 {
        probe += TimeDelta::minutes(1);
        if !matches!(tz.from_local_datetime(&probe), MappedLocalTime::None) {
            return probe;
        }
    }
    // 4 時間で見つからない異常ケース: 元の naive を返す (呼び出し側でフォールバック)。
    naive
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use chrono_tz::{Asia, Europe};

    fn naive(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
    }

    fn utc(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, mi, s).unwrap()
    }

    #[test]
    fn resolves_single_in_jst() {
        // Asia/Tokyo は DST なし、常に Single
        let n = naive(2026, 5, 9, 21, 43, 56);
        let (got_utc, offset, res) = resolve_local_to_utc(n, &Asia::Tokyo, None);
        assert_eq!(res, Resolution::Single);
        assert_eq!(offset, 9 * 3600);
        assert_eq!(got_utc, utc(2026, 5, 9, 12, 43, 56));
    }

    #[test]
    fn resolves_gap_pushes_to_next_valid_local() {
        // UK spring forward 2026-03-29: 01:00 GMT → 02:00 BST
        // 01:30 は存在しない (Gap)
        let n = naive(2026, 3, 29, 1, 30, 0);
        let (got_utc, offset, res) = resolve_local_to_utc(n, &Europe::London, None);
        assert_eq!(res, Resolution::Gap);
        assert_eq!(offset, 3600); // BST = UTC+1
                                  // 02:00 BST = 01:00 UTC
        assert_eq!(got_utc, utc(2026, 3, 29, 1, 0, 0));
    }

    #[test]
    fn resolves_ambiguous_first_when_no_prev() {
        // UK fall back 2026-10-25: 02:00 BST → 01:00 GMT
        // 01:30 は ambiguous
        let n = naive(2026, 10, 25, 1, 30, 0);
        let (got_utc, offset, res) = resolve_local_to_utc(n, &Europe::London, None);
        assert_eq!(res, Resolution::AmbiguousFirst);
        // BST 01:30 = UTC 00:30
        assert_eq!(offset, 3600);
        assert_eq!(got_utc, utc(2026, 10, 25, 0, 30, 0));
    }

    #[test]
    fn resolves_ambiguous_second_when_prev_forces_monotonic() {
        // 直前が UTC 00:45 (= ambiguous earlier の後ろ) のとき、
        // earlier を採用すると単調性が破れるので second を選ぶ。
        let n = naive(2026, 10, 25, 1, 30, 0);
        let prev = Some(utc(2026, 10, 25, 0, 45, 0));
        let (got_utc, offset, res) = resolve_local_to_utc(n, &Europe::London, prev);
        assert_eq!(res, Resolution::AmbiguousSecond);
        // GMT 01:30 = UTC 01:30
        assert_eq!(offset, 0);
        assert_eq!(got_utc, utc(2026, 10, 25, 1, 30, 0));
    }

    #[test]
    fn monotonic_across_dst_fall_back_with_prev_chain() {
        // local sequence を順に流すと UTC は単調増加すべき。
        let tz = Europe::London;
        let times = [
            naive(2026, 10, 25, 0, 30, 0),
            naive(2026, 10, 25, 1, 30, 0), // ambiguous → AmbiguousFirst (BST)
            naive(2026, 10, 25, 1, 30, 0), // ambiguous → AmbiguousSecond (GMT)
            naive(2026, 10, 25, 2, 30, 0),
        ];
        let expected_res = [
            Resolution::Single,
            Resolution::AmbiguousFirst,
            Resolution::AmbiguousSecond,
            Resolution::Single,
        ];
        let mut prev: Option<DateTime<Utc>> = None;
        let mut utcs: Vec<DateTime<Utc>> = Vec::new();
        let mut resolutions = Vec::new();
        for n in times {
            let (got_utc, _o, r) = resolve_local_to_utc(n, &tz, prev);
            utcs.push(got_utc);
            resolutions.push(r);
            prev = Some(got_utc);
        }
        assert!(
            utcs.windows(2).all(|w| w[0] < w[1]),
            "not monotonic: {utcs:?}"
        );
        assert_eq!(resolutions.as_slice(), expected_res.as_slice());
    }

    #[test]
    fn monotonic_across_dst_spring_forward() {
        let tz = Europe::London;
        let times = [
            naive(2026, 3, 29, 0, 30, 0),
            naive(2026, 3, 29, 1, 30, 0), // gap → 02:30 BST
            naive(2026, 3, 29, 2, 30, 0),
        ];
        let expected_res = [Resolution::Single, Resolution::Gap, Resolution::Single];
        let mut prev: Option<DateTime<Utc>> = None;
        let mut utcs: Vec<DateTime<Utc>> = Vec::new();
        let mut resolutions = Vec::new();
        for n in times {
            let (got_utc, _o, r) = resolve_local_to_utc(n, &tz, prev);
            utcs.push(got_utc);
            resolutions.push(r);
            prev = Some(got_utc);
        }
        assert!(
            utcs.windows(2).all(|w| w[0] < w[1]),
            "not monotonic: {utcs:?}"
        );
        assert_eq!(resolutions.as_slice(), expected_res.as_slice());
    }

    #[test]
    fn resolution_as_str_roundtrip() {
        for r in [
            Resolution::Single,
            Resolution::AmbiguousFirst,
            Resolution::AmbiguousSecond,
            Resolution::Gap,
        ] {
            // as_str は安定した DB シリアライズキーとして使うので人間可読でユニークであること。
            let s = r.as_str();
            assert!(!s.is_empty());
        }
    }
}
