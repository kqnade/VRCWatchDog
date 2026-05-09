//! Photo access ガード。
//!
//! Tauri command (`open_photo`/`open_photo_folder`) から呼ばれる前提で、
//! path traversal や予期しない拡張子の外部 open を防ぐ純粋関数群を提供する。

pub mod access;

pub use access::{validate_photo_path, PhotoAccessError, PhotoTarget};
