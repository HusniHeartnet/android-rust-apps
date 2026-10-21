//! Saved settings: one `key=value` per line in a text file, written whenever a value changes.

use std::collections::BTreeMap;
use std::path::PathBuf;

pub struct Settings {
    path: PathBuf,
    map: BTreeMap<String, String>,
}

impl Settings {
    /// Load `dir/name`; a missing or unreadable file just means defaults.
    pub fn load(dir: &std::path::Path, name: &str) -> Self {
        let path = dir.join(name);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        Self { path, map: parse(&text) }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(String::as_str)
    }

    pub fn bool(&self, key: &str, default: bool) -> bool {
        match self.get(key) {
            Some("1") | Some("true") => true,
            Some("0") | Some("false") => false,
            _ => default,
        }
    }

    pub fn int(&self, key: &str, default: i64) -> i64 {
        self.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
    }

    pub fn set(&mut self, key: &str, value: impl ToString) {
        let value = value.to_string().replace(['\n', '\r'], " ");
        if self.map.get(key) != Some(&value) {
            self.map.insert(key.to_string(), value);
            self.save();
        }
    }

    fn save(&self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text: String = self.map.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
        let tmp = self.path.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }
}

fn parse(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kit-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn values_survive_a_reload() {
        let d = dir("reload");
        let mut s = Settings::load(&d, "s.txt");
        s.set("size", 3);
        s.set("dark", true);
        let s = Settings::load(&d, "s.txt");
        assert_eq!(s.int("size", 1), 3);
        assert!(s.bool("dark", false));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn defaults_apply_when_missing_or_broken() {
        let d = dir("defaults");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("s.txt"), "size=abc\nnonsense\n=x\n").unwrap();
        let s = Settings::load(&d, "s.txt");
        assert_eq!(s.int("size", 7), 7);
        assert!(s.bool("missing", true));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn newlines_cannot_break_the_file() {
        let d = dir("newline");
        let mut s = Settings::load(&d, "s.txt");
        s.set("a", "one\ntwo");
        let s = Settings::load(&d, "s.txt");
        assert_eq!(s.get("a"), Some("one two"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
