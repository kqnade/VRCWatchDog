//! 写真 1 件を webp サムネに変換する純関数。
//!
//! `render_thumb_to_webp(input_path, max_dim)` が webp バイト列とその blake3 hex を返す。
//! actor (Phase 6.3.3) はこれを呼んで、結果バイトをファイルに書き、sha を DB に
//! 書き戻すだけ。
//!
//! ## なぜ webp?
//! - サムネ用途では PNG より 30-80% 小さい
//! - asset:// で配信するときに WebView2 がネイティブ対応している (追加デコード不要)
//!
//! ## なぜ lossless?
//! - `image` crate 0.25 は webp の lossless encoder のみ提供 (lossy は外部の
//!   `webp` crate 経由になる、追加 unsafe 依存)
//! - 256x256 程度の thumb なら lossless でも十数 KB / 件で済むので妥協可能
//! - lossy が必要になったら `webp` crate 切り替えを別 commit で

use std::io::Cursor;
use std::path::Path;

use image::ImageFormat;

/// `render_thumb_to_webp` のエラー。fs / decode / encode を区別して actor 側で
/// 適切な log level に振り分けられるようにしている。
#[derive(Debug, thiserror::Error)]
pub enum ThumbRenderError {
    #[error("could not open image file {0}: {1}")]
    Open(std::path::PathBuf, #[source] image::ImageError),
    #[error("could not decode image: {0}")]
    Decode(#[source] image::ImageError),
    #[error("could not encode webp: {0}")]
    Encode(#[source] image::ImageError),
}

/// `input_path` の画像を読み込み、長辺が `max_dim` 以内に収まるよう
/// アスペクト比を保ったままダウンサンプルし、webp バイト列に encode する。
///
/// 戻り値: `(webp_bytes, blake3_hex)`。
/// `blake3_hex` は webp_bytes 全体の blake3 (64 桁 hex)。同一画像 + 同一 max_dim なら
/// 同じ sha が返る (encoder の決定性に依存、image 0.25 の WebPEncoder は lossless で
/// 決定的)。
pub fn render_thumb_to_webp(
    input_path: &Path,
    max_dim: u32,
) -> Result<(Vec<u8>, String), ThumbRenderError> {
    // Step 1: open + decode
    let img = image::ImageReader::open(input_path)
        .map_err(|e| {
            ThumbRenderError::Open(input_path.to_path_buf(), image::ImageError::IoError(e))
        })?
        .with_guessed_format()
        .map_err(|e| {
            ThumbRenderError::Open(input_path.to_path_buf(), image::ImageError::IoError(e))
        })?
        .decode()
        .map_err(ThumbRenderError::Decode)?;

    // Step 2: thumbnail (アスペクト比保持、長辺 max_dim 以内)。
    // `image::DynamicImage::resize` は upscale もしてしまうので、
    // 元画像が既に max_dim 以下なら resize を skip して原寸そのまま返す
    // (圧縮の意味も無いが、無意味な拡大で画質劣化させるよりはマシ)。
    let thumb = if img.width() > max_dim || img.height() > max_dim {
        img.resize(max_dim, max_dim, image::imageops::FilterType::Triangle)
    } else {
        img
    };

    // Step 3: encode webp (lossless)
    let mut buf = Vec::new();
    thumb
        .write_to(&mut Cursor::new(&mut buf), ImageFormat::WebP)
        .map_err(ThumbRenderError::Encode)?;

    // Step 4: blake3 hex
    let sha = blake3::hash(&buf).to_hex().to_string();

    Ok((buf, sha))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};
    use std::path::PathBuf;
    use tempfile::tempdir;

    /// 指定サイズのテスト画像 (グラデーション) を PNG で書き出して path を返す。
    fn write_test_png(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_fn(w, h, |x, y| {
            // 単純なグラデーション。alpha は 255 固定。
            Rgba([
                (x * 255 / w.max(1)) as u8,
                (y * 255 / h.max(1)) as u8,
                128,
                255,
            ])
        });
        let path = dir.join(name);
        img.save_with_format(&path, ImageFormat::Png).unwrap();
        path
    }

    fn webp_signature_present(bytes: &[u8]) -> bool {
        // RIFF....WEBP のシグネチャ。RIFF (4) + size (4) + WEBP (4) = 先頭 12 bytes。
        bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
    }

    #[test]
    fn render_returns_webp_bytes_with_riff_webp_signature() {
        let dir = tempdir().unwrap();
        let png_path = write_test_png(dir.path(), "in.png", 800, 600);

        let (bytes, _sha) = render_thumb_to_webp(&png_path, 256).unwrap();

        assert!(
            webp_signature_present(&bytes),
            "出力は WebP シグネチャ (RIFF....WEBP) を持つ必要がある"
        );
    }

    #[test]
    fn render_produces_blake3_hex_of_64_chars() {
        let dir = tempdir().unwrap();
        let png_path = write_test_png(dir.path(), "in.png", 200, 100);

        let (_bytes, sha) = render_thumb_to_webp(&png_path, 256).unwrap();

        assert_eq!(sha.len(), 64, "blake3 256-bit hex は 64 文字");
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn render_is_deterministic_for_same_input_and_dim() {
        // image 0.25 の WebPEncoder は lossless で決定的。同入力で同じバイト + 同じ sha。
        let dir = tempdir().unwrap();
        let png_path = write_test_png(dir.path(), "in.png", 400, 400);

        let (b1, sha1) = render_thumb_to_webp(&png_path, 256).unwrap();
        let (b2, sha2) = render_thumb_to_webp(&png_path, 256).unwrap();

        assert_eq!(b1, b2);
        assert_eq!(sha1, sha2);
    }

    #[test]
    fn render_downsamples_image_so_long_side_does_not_exceed_max_dim() {
        // 1600x900 入力 → max_dim=320 で resize → 長辺 = 320
        let dir = tempdir().unwrap();
        let png_path = write_test_png(dir.path(), "wide.png", 1600, 900);

        let (bytes, _sha) = render_thumb_to_webp(&png_path, 320).unwrap();

        // 出力 webp を読み戻して dimension を assert する
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert!(
            decoded.width() <= 320 && decoded.height() <= 320,
            "long side <= max_dim, got {}x{}",
            decoded.width(),
            decoded.height()
        );
        // アスペクト比が保たれている (1600:900 ≒ 16:9 → 320:180)
        assert_eq!(
            decoded.width(),
            320,
            "long side (= width here) should hit max_dim"
        );
    }

    #[test]
    fn render_does_not_upscale_smaller_image() {
        // 100x100 入力 → max_dim=256。元より大きくしない (= そのまま 100x100)。
        let dir = tempdir().unwrap();
        let png_path = write_test_png(dir.path(), "small.png", 100, 100);

        let (bytes, _sha) = render_thumb_to_webp(&png_path, 256).unwrap();

        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!(decoded.width(), 100);
        assert_eq!(decoded.height(), 100);
    }

    #[test]
    fn render_returns_open_error_for_nonexistent_file() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("does_not_exist.png");

        let err = render_thumb_to_webp(&missing, 256).unwrap_err();

        match err {
            ThumbRenderError::Open(p, _) => assert_eq!(p, missing),
            other => panic!("expected Open error, got {other:?}"),
        }
    }

    #[test]
    fn render_returns_decode_error_for_garbage_bytes_in_png_named_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("not_actually_png.png");
        std::fs::write(&path, b"hello world this is not a png").unwrap();

        let err = render_thumb_to_webp(&path, 256).unwrap_err();

        // ImageReader::with_guessed_format → decode が失敗するので Decode に落ちる
        // (ファイル拡張子 .png から format guess するが、bytes が PNG header に
        // 該当しないのでデコード時点で reject)。
        assert!(matches!(err, ThumbRenderError::Decode(_)));
    }
}
