//! 写真ファイルへの「外部 open」要求を検証する純粋ロジック。
//!
//! Tauri 側の `open_photo` / `open_photo_folder` コマンドから呼び、検証に通った
//! 正規化済みパスのみ `app.shell().open()` に渡す。
//!
//! # 検証ステップ
//! 1. `photo_directory` を canonicalize (settings 起源・存在前提)
//! 2. `file_path` を canonicalize (symlink/junction を辿り終えた絶対パス)
//! 3. canonical file_path が `photo_directory` の subpath か確認
//! 4. ファイルの実体タイプ確認 (Photo→file, Folder→file の親 directory)
//! 5. 拡張子チェック (画像のみ)
//!
//! canonicalize は `..` を解決し、symlink を follow するため、
//! `Path::starts_with` のみによる検査では防げない攻撃を遮断できる。

use std::path::{Path, PathBuf};

/// `validate_photo_path` の検証対象。
///
/// `Photo` は写真ファイル本体を画像ビューワで開く用途。
/// `Folder` は Explorer で写真の親フォルダを開く用途。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoTarget {
    Photo,
    Folder,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PhotoAccessError {
    #[error("photo file not found: {0}")]
    NotFound(PathBuf),

    #[error("path is not a regular file: {0}")]
    NotAFile(PathBuf),

    #[error("invalid extension (must be png/jpg/jpeg/webp): {0}")]
    InvalidExtension(PathBuf),

    #[error("path is outside the configured photo_directory: file={file}, scope={scope}")]
    OutsideScope { file: PathBuf, scope: PathBuf },

    #[error("photo_directory not configured or not accessible: {0}")]
    ScopeNotAccessible(PathBuf),

    #[error("photo file has no parent directory: {0}")]
    NoParent(PathBuf),
}

const ALLOWED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

/// 外部 open に渡せるパスを返す。
///
/// 戻り値は target に応じて:
/// - `PhotoTarget::Photo` → canonicalize 済みの写真本体パス
/// - `PhotoTarget::Folder` → 写真本体の親 directory (canonicalize 済み)
///
/// # 失敗
/// ガードに失敗した場合、`PhotoAccessError` を返す。
/// 戻ったエラーは UI に表示してよいが、`file` フィールドの絶対パスは
/// 場合により秘密情報を含むため、ログ目的にのみ使うこと。
pub fn validate_photo_path(
    file_path: &Path,
    photo_directory: &Path,
    target: PhotoTarget,
) -> Result<PathBuf, PhotoAccessError> {
    // 1. scope を canonicalize。settings の photo_directory が存在しなければ即拒否。
    let scope = std::fs::canonicalize(photo_directory)
        .map_err(|_| PhotoAccessError::ScopeNotAccessible(photo_directory.to_path_buf()))?;

    // 2. file_path を canonicalize。存在しなければ NotFound。
    let canonical_file = std::fs::canonicalize(file_path)
        .map_err(|_| PhotoAccessError::NotFound(file_path.to_path_buf()))?;

    // 3. canonical_file が file/regular file か確認。
    let metadata = std::fs::metadata(&canonical_file)
        .map_err(|_| PhotoAccessError::NotFound(canonical_file.clone()))?;
    if !metadata.is_file() {
        return Err(PhotoAccessError::NotAFile(canonical_file));
    }

    // 4. 拡張子チェック。
    let ext_ok = canonical_file
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .map(|lower| ALLOWED_EXTENSIONS.contains(&lower.as_str()))
        .unwrap_or(false);
    if !ext_ok {
        return Err(PhotoAccessError::InvalidExtension(canonical_file));
    }

    // 5. canonical_file が scope 配下にあるか。`starts_with` は path component
    //    レベルでの比較なので、`/scopeX` と `/scope` を区別できる。
    if !canonical_file.starts_with(&scope) {
        return Err(PhotoAccessError::OutsideScope {
            file: canonical_file,
            scope,
        });
    }

    match target {
        PhotoTarget::Photo => Ok(canonical_file),
        PhotoTarget::Folder => canonical_file
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| PhotoAccessError::NoParent(canonical_file.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_scope() -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let scope = dir.path().to_path_buf();
        // canonicalize をテスト中で安定させるため scope も実際にディレクトリ作成
        (dir, scope)
    }

    fn write_image(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, b"\x89PNG\r\n\x1a\n").expect("write image");
        p
    }

    #[test]
    fn valid_png_inside_scope_returns_canonical_path() {
        let (_g, scope) = setup_scope();
        let file = write_image(&scope, "shot.png");
        let got = validate_photo_path(&file, &scope, PhotoTarget::Photo).expect("valid");
        assert_eq!(got, fs::canonicalize(&file).expect("canon"));
    }

    #[test]
    fn folder_target_returns_parent_directory() {
        let (_g, scope) = setup_scope();
        let sub = scope.join("2026-05");
        fs::create_dir_all(&sub).expect("mkdir");
        let file = write_image(&sub, "shot.jpg");
        let got = validate_photo_path(&file, &scope, PhotoTarget::Folder).expect("valid");
        assert_eq!(got, fs::canonicalize(&sub).expect("canon"));
    }

    #[test]
    fn jpeg_jpg_webp_all_allowed() {
        let (_g, scope) = setup_scope();
        for name in ["a.jpg", "b.jpeg", "c.webp", "d.PNG"] {
            let file = write_image(&scope, name);
            assert!(
                validate_photo_path(&file, &scope, PhotoTarget::Photo).is_ok(),
                "should allow {}",
                name
            );
        }
    }

    #[test]
    fn unknown_extension_rejected() {
        let (_g, scope) = setup_scope();
        let file = write_image(&scope, "evil.exe");
        let err = validate_photo_path(&file, &scope, PhotoTarget::Photo)
            .expect_err("expected validation error");
        assert!(matches!(err, PhotoAccessError::InvalidExtension(_)));
    }

    #[test]
    fn missing_extension_rejected() {
        let (_g, scope) = setup_scope();
        let file = write_image(&scope, "noext");
        let err = validate_photo_path(&file, &scope, PhotoTarget::Photo)
            .expect_err("expected validation error");
        assert!(matches!(err, PhotoAccessError::InvalidExtension(_)));
    }

    #[test]
    fn nonexistent_file_rejected() {
        let (_g, scope) = setup_scope();
        let missing = scope.join("ghost.png");
        let err = validate_photo_path(&missing, &scope, PhotoTarget::Photo)
            .expect_err("expected validation error");
        assert!(matches!(err, PhotoAccessError::NotFound(_)));
    }

    #[test]
    fn directory_passed_as_photo_rejected() {
        let (_g, scope) = setup_scope();
        let sub = scope.join("subdir");
        fs::create_dir_all(&sub).expect("mkdir");
        let err = validate_photo_path(&sub, &scope, PhotoTarget::Photo)
            .expect_err("expected validation error");
        // canonicalize はディレクトリでも成功するが NotAFile が返る (拡張子チェックも兼ねる)
        assert!(matches!(
            err,
            PhotoAccessError::NotAFile(_) | PhotoAccessError::InvalidExtension(_)
        ));
    }

    #[test]
    fn path_traversal_via_dotdot_rejected() {
        // scope/inner と scope/outside 兄弟を作る。inner から ../outside/foo.png
        // を渡されると canonicalize が `outside` に解決し scope.starts_with で弾く想定。
        // ただし canonicalize は scope/inner からの相対 ../outside を絶対パス化するので
        // スコープの subpath にならない。
        let (_g, scope) = setup_scope();
        let inner = scope.join("inner");
        let outside_dir = scope.parent().expect("parent of scope").join("outside_dir");
        fs::create_dir_all(&inner).expect("mkdir inner");
        fs::create_dir_all(&outside_dir).expect("mkdir outside");
        let outside_file = write_image(&outside_dir, "leak.png");

        let traversal = inner.join("..").join("..").join(
            outside_dir
                .file_name()
                .expect("outside name")
                .to_str()
                .expect("utf8"),
        );
        let traversal = traversal.join("leak.png");

        let err = validate_photo_path(&traversal, &scope, PhotoTarget::Photo)
            .expect_err("expected validation error");
        // canonicalize により outside_dir/leak.png に解決され、scope.starts_with で弾かれる。
        assert!(
            matches!(err, PhotoAccessError::OutsideScope { .. }),
            "expected OutsideScope, got {err:?}"
        );

        // 後始末
        let _ = fs::remove_file(outside_file);
        let _ = fs::remove_dir(&outside_dir);
    }

    #[test]
    fn scope_directory_missing_rejected() {
        let (_g, scope) = setup_scope();
        let file = write_image(&scope, "ok.png");
        let bogus_scope = scope.join("does_not_exist");
        let err = validate_photo_path(&file, &bogus_scope, PhotoTarget::Photo)
            .expect_err("expected validation error");
        assert!(matches!(err, PhotoAccessError::ScopeNotAccessible(_)));
    }

    #[test]
    fn similar_prefix_directory_does_not_match_scope() {
        // /tmp/xxx_scope と /tmp/xxx_scope_evil を兄弟で作り、後者の写真を渡されたとき
        // starts_with(scope) が path component 単位なので false を返すことを保証。
        let parent = TempDir::new().expect("parent");
        let scope = parent.path().join("xxx_scope");
        let evil = parent.path().join("xxx_scope_evil");
        fs::create_dir_all(&scope).expect("mkdir scope");
        fs::create_dir_all(&evil).expect("mkdir evil");
        let evil_file = write_image(&evil, "leak.png");

        let err = validate_photo_path(&evil_file, &scope, PhotoTarget::Photo)
            .expect_err("expected validation error");
        assert!(matches!(err, PhotoAccessError::OutsideScope { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escaping_scope_rejected() {
        use std::os::unix::fs as unix_fs;

        let parent = TempDir::new().expect("parent");
        let scope = parent.path().join("scope");
        let outside = parent.path().join("outside");
        fs::create_dir_all(&scope).expect("mkdir scope");
        fs::create_dir_all(&outside).expect("mkdir outside");
        let real = write_image(&outside, "secret.png");

        let link = scope.join("trap.png");
        unix_fs::symlink(&real, &link).expect("symlink");

        let err = validate_photo_path(&link, &scope, PhotoTarget::Photo)
            .expect_err("expected validation error");
        // canonicalize が symlink を follow して outside/secret.png に解決する。
        assert!(matches!(err, PhotoAccessError::OutsideScope { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_within_scope_allowed() {
        use std::os::unix::fs as unix_fs;

        let (_g, scope) = setup_scope();
        let real = write_image(&scope, "real.png");
        let link = scope.join("alias.png");
        unix_fs::symlink(&real, &link).expect("symlink");

        let got = validate_photo_path(&link, &scope, PhotoTarget::Photo).expect("valid");
        assert_eq!(got, fs::canonicalize(&real).expect("canon"));
    }
}
