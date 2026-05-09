//! ログファイルの物理同一性を表す [`FileIdentity`] と、generation bump 判定。
//!
//! 5 フィールドから blake3 ハッシュを生成して `processed_log_files` の
//! `file_identity_hash` (UNIQUE) として保存する。
//!
//! generation は以下のいずれかが起きると bump (++):
//! - truncate (`current_size < ingest_position`)
//! - 先頭 1KB の hash 変化 (overwrite)
//! - creation_time 変化 (delete + recreate で新 inode)
//! - mtime 逆行 (clock 巻戻し相当)

use serde::{Deserialize, Serialize};

/// ログファイルの物理アイデンティティ。
///
/// Windows では `volume_serial` + `file_id` が一意 (NTFS の `FILE_ID_INFO`)。
/// Linux/macOS のテストでは `volume_serial = device id`、`file_id = inode` を使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileIdentity {
    pub volume_serial: u32,
    pub file_id_high: u32,
    pub file_id_low: u32,
    pub generation: u32,
    /// FILETIME 互換 (100-ns since 1601-01-01 UTC)、Linux では proxy として
    /// `st_birthtime` あるいは `st_ctime` を 100-ns 単位で渡す。
    pub creation_time: i64,
    /// 先頭 1KB の blake3 ハッシュ (32 byte)。
    pub first_kb_hash: [u8; 32],
}

impl FileIdentity {
    /// `processed_log_files.file_identity_hash` 用の hex 文字列を生成する。
    pub fn identity_hash_hex(&self) -> String {
        compute_file_identity_hash(self)
    }
}

/// generation を bump する原因。テスト・診断用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationBumpReason {
    Truncate,
    FirstKbHashChanged,
    CreationTimeChanged,
    MtimeWentBackwards,
}

/// `FileIdentity` の 5 フィールド + generation を blake3 して hex 化する。
pub fn compute_file_identity_hash(id: &FileIdentity) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&id.volume_serial.to_le_bytes());
    hasher.update(&id.file_id_high.to_le_bytes());
    hasher.update(&id.file_id_low.to_le_bytes());
    hasher.update(&id.generation.to_le_bytes());
    hasher.update(&id.creation_time.to_le_bytes());
    hasher.update(&id.first_kb_hash);
    hex_encode(hasher.finalize().as_bytes())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// DB から復元した既知の状態 (`PrevFileState`) とファイルシステムから読んだ
/// 最新の状態 (`CurrentFileState`) を保持して [`detect_bump`] に渡す。
#[derive(Debug, Clone, Copy)]
pub struct PrevFileState<'a> {
    pub generation: u32,
    pub first_kb_hash: &'a [u8; 32],
    pub creation_time: i64,
    pub mtime_unix_secs: i64,
    pub ingest_position: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct CurrentFileState<'a> {
    pub size: i64,
    pub first_kb_hash: &'a [u8; 32],
    pub creation_time: i64,
    pub mtime_unix_secs: i64,
}

/// 既知の `prev` (DB から復元) と `current` (ファイルシステムから読んだ最新) を比較し、
/// generation を bump する必要があるかを判定する。
///
/// `prev` は同 `(volume_serial, file_id_high, file_id_low)` で `MAX(generation)` の行を想定。
/// 戻り値は (推奨される新 generation, bump 理由)。bump 不要なら `None`。
pub fn detect_bump(
    prev: PrevFileState<'_>,
    current: CurrentFileState<'_>,
) -> Option<(u32, GenerationBumpReason)> {
    if current.size < prev.ingest_position {
        return Some((prev.generation + 1, GenerationBumpReason::Truncate));
    }
    if current.first_kb_hash != prev.first_kb_hash {
        return Some((
            prev.generation + 1,
            GenerationBumpReason::FirstKbHashChanged,
        ));
    }
    if current.creation_time != prev.creation_time {
        return Some((
            prev.generation + 1,
            GenerationBumpReason::CreationTimeChanged,
        ));
    }
    if current.mtime_unix_secs < prev.mtime_unix_secs {
        return Some((
            prev.generation + 1,
            GenerationBumpReason::MtimeWentBackwards,
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(seed: u8) -> [u8; 32] {
        [seed; 32]
    }

    #[test]
    fn identity_hash_is_deterministic_and_unique_per_field() {
        let a = FileIdentity {
            volume_serial: 1,
            file_id_high: 2,
            file_id_low: 3,
            generation: 0,
            creation_time: 100,
            first_kb_hash: h(0xAA),
        };
        assert_eq!(a.identity_hash_hex(), a.identity_hash_hex());

        // generation が変わると hash は変わる
        let mut b = a;
        b.generation = 1;
        assert_ne!(a.identity_hash_hex(), b.identity_hash_hex());

        // first_kb_hash が変わると hash は変わる
        let mut c = a;
        c.first_kb_hash = h(0xBB);
        assert_ne!(a.identity_hash_hex(), c.identity_hash_hex());
    }

    fn prev(
        generation: u32,
        first_kb: &[u8; 32],
        creation_time: i64,
        mtime: i64,
        ingest_position: i64,
    ) -> PrevFileState<'_> {
        PrevFileState {
            generation,
            first_kb_hash: first_kb,
            creation_time,
            mtime_unix_secs: mtime,
            ingest_position,
        }
    }

    fn curr(
        size: i64,
        first_kb: &[u8; 32],
        creation_time: i64,
        mtime: i64,
    ) -> CurrentFileState<'_> {
        CurrentFileState {
            size,
            first_kb_hash: first_kb,
            creation_time,
            mtime_unix_secs: mtime,
        }
    }

    #[test]
    fn detect_truncate() {
        let p_kb = h(0xAA);
        let c_kb = h(0xAA);
        let r = detect_bump(prev(0, &p_kb, 100, 1000, 5000), curr(100, &c_kb, 100, 1000));
        assert_eq!(r, Some((1, GenerationBumpReason::Truncate)));
    }

    #[test]
    fn detect_first_kb_hash_change() {
        let p_kb = h(0xAA);
        let c_kb = h(0xBB);
        let r = detect_bump(prev(0, &p_kb, 100, 1000, 0), curr(5000, &c_kb, 100, 1000));
        assert_eq!(r, Some((1, GenerationBumpReason::FirstKbHashChanged)));
    }

    #[test]
    fn detect_creation_time_change() {
        let kb = h(0xAA);
        let r = detect_bump(prev(0, &kb, 100, 1000, 0), curr(5000, &kb, 200, 1000));
        assert_eq!(r, Some((1, GenerationBumpReason::CreationTimeChanged)));
    }

    #[test]
    fn detect_mtime_backwards() {
        let kb = h(0xAA);
        let r = detect_bump(prev(0, &kb, 100, 2000, 0), curr(5000, &kb, 100, 1000));
        assert_eq!(r, Some((1, GenerationBumpReason::MtimeWentBackwards)));
    }

    #[test]
    fn no_bump_when_file_is_unchanged_or_just_grew() {
        // size が大きくなっただけ (通常追記)、hash/creation/mtime は同じ
        let kb = h(0xAA);
        let r = detect_bump(prev(3, &kb, 100, 1000, 500), curr(5000, &kb, 100, 1500));
        assert_eq!(r, None);
    }

    #[test]
    fn truncate_takes_priority_over_other_changes() {
        // size < ingest_position なので truncate が優先
        let p_kb = h(0xAA);
        let c_kb = h(0xBB);
        let r = detect_bump(prev(0, &p_kb, 100, 1000, 5000), curr(0, &c_kb, 200, 500));
        assert_eq!(r, Some((1, GenerationBumpReason::Truncate)));
    }
}
