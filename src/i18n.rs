//! The interface language. Strings are written in Russian in the code;
//! [`tr`] gives their English text from [`EN`] when the interface is in
//! English. Templates use `{}` (in order) or `{0}`, `{1}` (by position) and
//! are filled by [`t!`](crate::t).

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

mod en;
use en::EN;

/// The language setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// As in Windows: Russian for a Russian Windows, English otherwise.
    #[default]
    Auto,
    Ru,
    En,
}

static ENGLISH: AtomicBool = AtomicBool::new(false);

/// Sets the interface language.
pub fn init(setting: Language) {
    let english = match setting {
        Language::Auto => !windows_is_russian(),
        Language::Ru => false,
        Language::En => true,
    };
    ENGLISH.store(english, Ordering::Relaxed);
}

/// `true` when the interface is in English.
pub fn english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

fn windows_is_russian() -> bool {
    // SAFETY: no preconditions.
    let lang = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    lang & 0x3ff == 0x19 // LANG_RUSSIAN
}

fn table() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| EN.iter().copied().collect())
}

/// `ru` in the interface language.
pub fn tr(ru: &'static str) -> &'static str {
    if !english() {
        return ru;
    }
    table().get(ru).copied().unwrap_or(ru)
}

/// A template with `{}` / `{N}` placeholders filled with `args`; `{{` and
/// `}}` are literal braces.
pub fn fill(template: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut next = 0;
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if let Some(end) = tail.strip_prefix('{').and_then(|t| t.find('}')) {
            let inner = &tail[1..=end];
            let index = if inner.is_empty() {
                next += 1;
                Some(next - 1)
            } else {
                inner.parse::<usize>().ok()
            };
            if let Some(a) = index.and_then(|k| args.get(k)) {
                out.push_str(&a.to_string());
                rest = &tail[end + 2..];
                continue;
            }
        }
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// A number with `digits` decimals: a decimal comma in Russian, a point
/// in English.
pub fn dec(v: impl Into<f64>, digits: usize) -> String {
    let s = format!("{:.*}", digits, v.into());
    if english() { s } else { s.replace('.', ",") }
}

/// `tr` on a template, then the arguments filled in:
/// `t!("Слой {} из {}", a, b)`.
#[macro_export]
macro_rules! t {
    ($s:literal $(, $a:expr)* $(,)?) => {
        $crate::i18n::fill($crate::i18n::tr($s), &[$(&$a as &dyn ::std::fmt::Display),*])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_templates() {
        assert_eq!(fill("{} из {}", &[&3, &12]), "3 из 12");
        assert_eq!(fill("{1}, {0}", &[&"a", &"b"]), "b, a");
        assert_eq!(fill("{{n}} {}", &[&5]), "{n} 5");
        assert_eq!(fill("no args {}", &[]), "no args {}");
    }

    /// Russian literals outside tests and comments, as written in the
    /// source (escapes resolved).
    fn russian_literals() -> Vec<(String, String)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        let mut dirs = vec![root];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    files.push(p);
                }
            }
        }
        let mut out = Vec::new();
        for f in files {
            if f.ends_with("en.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&f).unwrap();
            // Test modules are skipped: from `#[cfg(test)]` to the end of
            // the item after it (by brace depth).
            let mut skip_depth: Option<i32> = None;
            let mut depth = 0i32;
            let mut armed = false;
            for (n, line) in text.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("#[cfg(test)]") {
                    armed = true;
                    continue;
                }
                let opens = line.matches('{').count() as i32;
                let closes = line.matches('}').count() as i32;
                if armed && opens > 0 {
                    skip_depth = Some(depth);
                    armed = false;
                }
                depth += opens - closes;
                if let Some(d) = skip_depth {
                    if depth <= d {
                        skip_depth = None;
                    }
                    continue;
                }
                if code.starts_with("//") {
                    continue;
                }
                for lit in literals(line) {
                    if lit.chars().any(|c| {
                        ('а'..='я').contains(&c.to_ascii_lowercase())
                            || ('А'..='Я').contains(&c)
                            || c == 'ё'
                            || c == 'Ё'
                    }) {
                        out.push((format!("{}:{}", f.display(), n + 1), lit));
                    }
                }
            }
        }
        out
    }

    /// String literals on one line, unescaped.
    fn literals(line: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut chars = line.chars().peekable();
        let mut prev = ' ';
        while let Some(c) = chars.next() {
            // Skip char literals such as '"'.
            if c == '\'' && chars.peek() == Some(&'"') {
                chars.next();
                prev = '"';
                continue;
            }
            if c == '/' && prev == '/' {
                break;
            }
            if c != '"' {
                prev = c;
                continue;
            }
            let mut s = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some('n') => s.push('\n'),
                        Some('t') => s.push('\t'),
                        Some(o) => s.push(o),
                        None => {}
                    },
                    c => s.push(c),
                }
            }
            out.push(s);
            prev = ' ';
        }
        out
    }

    /// Every Russian string in the code has an English text in the table,
    /// with the same placeholders.
    #[test]
    fn every_string_is_translated() {
        let table = table();
        let mut missing = Vec::new();
        for (at, lit) in russian_literals() {
            match table.get(lit.as_str()) {
                None => missing.push(format!("{at}: {lit:?}")),
                Some(en) => {
                    let count = |s: &str| s.matches("{}").count();
                    assert_eq!(count(&lit), count(en), "{at}: {lit:?} → {en:?}");
                }
            }
        }
        assert!(
            missing.is_empty(),
            "no English for:\n{}",
            missing.join("\n")
        );
    }

    /// No table entry is left over from strings that are gone.
    #[test]
    fn no_unused_translations() {
        let used: std::collections::HashSet<String> =
            russian_literals().into_iter().map(|(_, l)| l).collect();
        let unused: Vec<_> = EN.iter().filter(|(ru, _)| !used.contains(*ru)).collect();
        assert!(unused.is_empty(), "unused: {unused:?}");
        assert_eq!(table().len(), EN.len(), "a string is in the table twice");
    }
}
