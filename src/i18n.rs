//! Russian or English, by the setting or the system's language.

use std::sync::atomic::{AtomicBool, Ordering};

static RU: AtomicBool = AtomicBool::new(true);

pub fn system_is_ru() -> bool {
    ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty())).is_none_or(|l| l.starts_with("ru"))
}

pub fn set_ru(ru: bool) {
    RU.store(ru, Ordering::Relaxed);
}

pub fn is_ru() -> bool {
    RU.load(Ordering::Relaxed)
}

/// The text in the language in use.
pub fn t(ru: &'static str, en: &'static str) -> &'static str {
    if is_ru() { ru } else { en }
}
