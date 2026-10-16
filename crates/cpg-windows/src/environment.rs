//! Unicode environment blocks for `CreateProcessW`: case-insensitive
//! variable identity, ordinal-sorted canonical blocks with duplicate
//! collapsing, and `=C:` drive-current-directory entries excluded.

use cpg_domain::{Error, Result};
use windows::Win32::System::Environment::{FreeEnvironmentStringsW, GetEnvironmentStringsW};
use windows::core::PCWSTR;

#[derive(Clone, Debug, Default)]
pub struct UnicodeEnvironment {
    entries: Vec<(String, String)>,
    block: Vec<u16>,
}

fn ordinal_compare_ci(left: &str, right: &str) -> std::cmp::Ordering {
    // CompareStringOrdinal(..., TRUE) compares UTF-16 code units
    // case-insensitively; for environment variable names (arbitrary UTF-16
    // except '=') this equals a per-unit ASCII-case fold plus raw ordering.
    let mut left = left.encode_utf16();
    let mut right = right.encode_utf16();
    loop {
        match (left.next(), right.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(l), Some(r)) => {
                let lf = fold_unit(l);
                let rf = fold_unit(r);
                if lf != rf {
                    return lf.cmp(&rf);
                }
            }
        }
    }
}

fn fold_unit(unit: u16) -> u16 {
    // Ordinal uppercase fold for ASCII and Latin-1, matching
    // CompareStringOrdinal's simple case folding for these ranges.
    match unit {
        0x41..=0x5A => unit + 32, // 'A'..='Z'
        0xC0..=0xDE if unit != 0xD7 => unit + 32,
        _ => unit,
    }
}

fn iequals(left: &str, right: &str) -> bool {
    ordinal_compare_ci(left, right) == std::cmp::Ordering::Equal
}

impl UnicodeEnvironment {
    pub fn current() -> Result<UnicodeEnvironment> {
        // SAFETY: GetEnvironmentStringsW returns a CoTaskMem-owned block that
        // we walk by null-terminated entries and free exactly once.
        let block = unsafe { GetEnvironmentStringsW() };
        if block.is_null() {
            return Err(Error::new(
                "ENVIRONMENT_UNAVAILABLE",
                "Cannot read the process environment.",
            ));
        }
        let mut environment = UnicodeEnvironment::default();
        unsafe {
            let mut cursor = block.0;
            while *cursor != 0 {
                let mut length = 0usize;
                while *cursor.add(length) != 0 {
                    length += 1;
                }
                let slice = std::slice::from_raw_parts(cursor, length);
                let text = String::from_utf16_lossy(slice);
                if let Some(equal) = text.find('=')
                    // Hidden drive-state entries ("=C:=…") have an empty name
                    // and are deliberately excluded.
                    && equal != 0
                {
                    environment.set(&text[..equal], &text[equal + 1..]);
                }
                cursor = cursor.add(length + 1);
            }
            // SAFETY: freeing the block returned by GetEnvironmentStringsW.
            let _ = FreeEnvironmentStringsW(block);
        }
        Ok(environment)
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.validate_entry(key, value).expect("valid entry");
        if let Some(entry) = self.entries.iter_mut().find(|(name, _)| iequals(name, key)) {
            entry.1 = value.to_owned();
        } else {
            self.entries.push((key.to_owned(), value.to_owned()));
        }
        self.block.clear();
    }

    pub fn remove(&mut self, key: &str) {
        let before = self.entries.len();
        self.entries.retain(|(name, _)| !iequals(name, key));
        if self.entries.len() != before {
            self.block.clear();
        }
    }

    pub fn has(&self, key: &str) -> bool {
        self.value(key).is_some()
    }

    pub fn value(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(name, _)| iequals(name, key))
            .map(|(_, value)| value.as_str())
    }

    /// Canonical `CreateProcessW` environment block: ordinal-sorted
    /// case-insensitively, duplicate name spellings collapsed to the first
    /// occurrence, terminated by the final double NUL.
    pub fn block(&mut self) -> &[u16] {
        if self.block.is_empty() {
            let mut sorted: Vec<&(String, String)> = self.entries.iter().collect();
            sorted.sort_by(|a, b| ordinal_compare_ci(&a.0, &b.0));
            let mut block = Vec::<u16>::new();
            let mut previous: Option<&str> = None;
            for entry in sorted {
                if let Some(previous) = previous
                    && iequals(previous, &entry.0)
                {
                    continue; // same name, different spelling
                }
                block.extend(entry.0.encode_utf16());
                block.push('=' as u16);
                block.extend(entry.1.encode_utf16());
                block.push(0);
                previous = Some(entry.0.as_str());
            }
            block.push(0);
            if block.len() == 1 {
                block.push(0);
            }
            self.block = block;
        }
        self.block.as_slice()
    }

    fn validate_entry(&self, key: &str, value: &str) -> Result<()> {
        if key.is_empty() || key.contains('=') || key.contains('\0') || value.contains('\0') {
            return Err(Error::new(
                "ENVIRONMENT_INVALID",
                "Invalid environment entry",
            ));
        }
        Ok(())
    }

    pub fn pcwstr_block(&mut self) -> PCWSTR {
        PCWSTR(self.block().as_ptr())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_replaces_case_variants_and_removes() {
        let mut environment = UnicodeEnvironment::default();
        environment.set("PATH", "a");
        environment.set("path", "b");
        assert_eq!(environment.entries.len(), 1);
        assert_eq!(environment.value("Path"), Some("b"));
        environment.remove("PATH");
        assert!(!environment.has("path"));
    }

    #[test]
    fn block_is_sorted_with_double_nul() {
        let mut environment = UnicodeEnvironment::default();
        environment.set("zeta", "1");
        environment.set("ALPHA", "2");
        environment.set("beta", "3");
        environment.set("alpha", "4"); // replaces the value, keeps first spelling
        let block = environment.block();
        assert!(block.ends_with(&[0, 0]));
        let text = String::from_utf16_lossy(&block[..block.len() - 2]);
        let names: Vec<&str> = text
            .split('\0')
            .map(|e| e.split('=').next().unwrap_or(""))
            .collect();
        assert_eq!(names, ["ALPHA", "beta", "zeta"]);
        assert!(
            text.contains("ALPHA=4"),
            "set replaces the value under the first spelling"
        );
    }

    #[test]
    fn current_environment_reads() {
        let environment = UnicodeEnvironment::current().unwrap();
        assert!(environment.has("PATH") || environment.has("Path"));
        assert!(environment.has("SYSTEMROOT") || environment.has("SystemRoot"));
    }

    #[test]
    fn empty_block_is_valid_double_nul() {
        let mut environment = UnicodeEnvironment::default();
        assert_eq!(environment.block(), &[0, 0]);
    }
}
