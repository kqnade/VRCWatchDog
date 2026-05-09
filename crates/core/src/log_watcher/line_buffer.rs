//! 不完全行を次回 ingest まで保持する byte buffer。
//!
//! Codex review の指摘 (Issue #2 修正): `BufReader::read_line` は EOF で
//! 改行未到達の不完全行を返してしまう。本実装は `read_until(b'\n')` 相当の
//! セマンティクスをサーフェスし、**改行が確認できた行のみ**を呼び出し側に
//! 渡す。残りバイトは buffer に保持し、次回追加 read で結合する。

/// 「完了行」(末尾が `\n` で終わる行) のみを切り出す byte buffer。
///
/// 内部に未完了の trailing bytes を保持する。`take_completed_lines` を呼ぶたびに
/// 完了済み行が消費される。各行は trailing `\n`/`\r` を取り除いた `String` を返す
/// (lossy: 不正 UTF-8 は U+FFFD に置き換える)。
#[derive(Debug, Default)]
pub struct LineBuffer {
    pending: Vec<u8>,
    /// `take_completed_lines` 呼び出し時に切り出した最後の完了行末尾の
    /// 「ファイル先頭からのバイト位置」。呼び出し側は前回の base byte_offset と
    /// 合わせて raw_log_events.byte_offset を計算する。
    consumed_bytes: u64,
}

impl LineBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// BOM (UTF-8 EF BB BF) を読み飛ばす (ファイル冒頭で 1 度呼ぶ想定)。
    /// 既に何か読み込んでいる場合は何もしない。
    pub fn skip_utf8_bom(&mut self) {
        if self.pending.starts_with(&[0xEF, 0xBB, 0xBF]) {
            self.pending.drain(..3);
            self.consumed_bytes += 3;
        }
    }

    /// 新しいバイトを buffer に追加する。
    pub fn extend_from_slice(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
    }

    /// 完了行 (`\n` で終わる行) を全て取り出して返す。
    ///
    /// 戻り値は `(line_string, byte_offset_after_line)` のリスト。
    /// `byte_offset_after_line` はファイル先頭からその行の改行直後までのバイト数で、
    /// `last_line.byte_offset_after_line` を `processed_log_files.ingest_position` に
    /// 保存することで「完了行の末尾」を cursor として記録できる。
    pub fn take_completed_lines(&mut self) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        let mut start = 0usize;
        for (i, b) in self.pending.iter().enumerate() {
            if *b == b'\n' {
                let raw = &self.pending[start..i]; // \n 手前まで
                let trimmed_end = if raw.last() == Some(&b'\r') {
                    &raw[..raw.len() - 1]
                } else {
                    raw
                };
                let s = String::from_utf8_lossy(trimmed_end).into_owned();
                let after_lf = (i + 1) as u64;
                let new_consumed = self.consumed_bytes + after_lf;
                out.push((s, new_consumed));
                start = i + 1;
            }
        }
        if start > 0 {
            self.consumed_bytes += start as u64;
            self.pending.drain(..start);
        }
        out
    }

    /// 現在の cursor (= 最後に completed line を切り出した位置)。
    /// 不完全行 (まだ `\n` を見ていないバイト) はここに含めない。
    pub fn completed_cursor(&self) -> u64 {
        self.consumed_bytes
    }

    /// 不完全行として buffer に滞留しているバイト数。診断用。
    pub fn pending_bytes(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_line_emitted_with_correct_offset() {
        let mut buf = LineBuffer::new();
        buf.extend_from_slice(b"hello\nworld\n");
        let lines = buf.take_completed_lines();
        assert_eq!(lines, vec![("hello".into(), 6), ("world".into(), 12)]);
        assert_eq!(buf.completed_cursor(), 12);
        assert_eq!(buf.pending_bytes(), 0);
    }

    #[test]
    fn incomplete_trailing_line_held_until_next_extend() {
        // Issue #2 の中核ケース: EOF で改行未到達 → 次回 read で完成
        let mut buf = LineBuffer::new();
        buf.extend_from_slice(b"hello\nincomp");
        let lines = buf.take_completed_lines();
        assert_eq!(lines, vec![("hello".into(), 6)]);
        assert_eq!(buf.completed_cursor(), 6);
        assert_eq!(buf.pending_bytes(), 6);

        // 次の read で改行が来る → 完成
        buf.extend_from_slice(b"lete\n");
        let lines = buf.take_completed_lines();
        assert_eq!(lines, vec![("incomplete".into(), 17)]);
        assert_eq!(buf.completed_cursor(), 17);
        assert_eq!(buf.pending_bytes(), 0);
    }

    #[test]
    fn crlf_endings_normalized() {
        let mut buf = LineBuffer::new();
        buf.extend_from_slice(b"hello\r\nworld\r\n");
        let lines = buf.take_completed_lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].0, "hello");
        assert_eq!(lines[1].0, "world");
        // cursor は CRLF 込みのバイト数なので 7 + 7 = 14
        assert_eq!(buf.completed_cursor(), 14);
    }

    #[test]
    fn utf8_bom_skipped_at_start() {
        let mut buf = LineBuffer::new();
        buf.extend_from_slice(b"\xEF\xBB\xBFhello\n");
        buf.skip_utf8_bom();
        let lines = buf.take_completed_lines();
        assert_eq!(lines, vec![("hello".into(), 9)]);
        // 6 (hello\n) + 3 (BOM) = 9
        assert_eq!(buf.completed_cursor(), 9);
    }

    #[test]
    fn invalid_utf8_replaced_lossily() {
        let mut buf = LineBuffer::new();
        // 0xFF は UTF-8 として不正
        buf.extend_from_slice(b"abc\xFF\xFEdef\n");
        let lines = buf.take_completed_lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].0.contains("abc"));
        assert!(lines[0].0.contains("def"));
        // 不正バイトは U+FFFD に置換されているはず
        assert!(lines[0].0.contains('\u{FFFD}'));
    }

    #[test]
    fn empty_lines_preserved() {
        let mut buf = LineBuffer::new();
        buf.extend_from_slice(b"\n\nfoo\n");
        let lines = buf.take_completed_lines();
        assert_eq!(
            lines,
            vec![("".into(), 1), ("".into(), 2), ("foo".into(), 6)]
        );
    }

    #[test]
    fn cursor_advances_only_on_completed_lines() {
        let mut buf = LineBuffer::new();
        buf.extend_from_slice(b"partial");
        let lines = buf.take_completed_lines();
        assert_eq!(lines, vec![]);
        assert_eq!(buf.completed_cursor(), 0);
        assert_eq!(buf.pending_bytes(), 7);
    }

    #[test]
    fn burst_of_short_lines_handled_in_single_take() {
        let mut buf = LineBuffer::new();
        for i in 0..100 {
            buf.extend_from_slice(format!("line{i}\n").as_bytes());
        }
        let lines = buf.take_completed_lines();
        assert_eq!(lines.len(), 100);
        assert_eq!(lines[99].0, "line99");
    }
}
