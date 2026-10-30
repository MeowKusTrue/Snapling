pub mod annotate;
pub mod config;
pub mod history;
pub mod hotkey;
pub mod i18n;
pub mod ocr;
pub mod record;
pub mod selection;
pub mod sound;
pub mod storage;

pub const APP_NAME: &str = "Snapling";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
