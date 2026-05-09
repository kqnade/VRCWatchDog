//! VRChat ログのパース。
//!
//! 意味イベントは [`LogEvent`] にマップし、解釈不能な行は
//! [`LogEvent::UnparsableLine`] に落として raw 永続化に回す。

mod event;
mod log_filename;
mod parse;

pub use event::{LogEvent, ParsedLine};
pub use log_filename::derive_log_sequence_key;
pub use parse::parse_line;
