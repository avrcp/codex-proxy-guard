//! Offline license notices streamed by the `licenses` command. Notice texts
//! are embedded at compile time so the single EXE stays self-contained.

struct Notice {
    name: &'static str,
    text: &'static str,
}

const NOTICES: &[Notice] = &[
    Notice {
        name: "LICENSE (codex-proxy-guard)",
        text: include_str!("../../LICENSE"),
    },
    Notice {
        name: "ratatui",
        text: include_str!("../../third-party-licenses/ratatui.txt"),
    },
    Notice {
        name: "crossterm",
        text: include_str!("../../third-party-licenses/crossterm.txt"),
    },
    Notice {
        name: "toml",
        text: include_str!("../../third-party-licenses/toml.txt"),
    },
    Notice {
        name: "windows-rs",
        text: include_str!("../../third-party-licenses/windows-rs.txt"),
    },
];

/// Streams `===== name =====`-separated notices to stdout; returns the
/// process exit code.
pub fn stream() -> i32 {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for notice in NOTICES {
        let header = format!("===== {} =====\n", notice.name);
        if out.write_all(header.as_bytes()).is_err() {
            return 1;
        }
        if out.write_all(notice.text.as_bytes()).is_err() {
            return 1;
        }
        if !notice.text.ends_with('\n') {
            let _ = out.write_all(b"\n");
        }
        let _ = out.write_all(b"\n-----\n\n");
    }
    0
}

#[allow(dead_code)]
/// The sorted embedded notice set, exposed for tests.
pub fn notice_names() -> Vec<&'static str> {
    NOTICES.iter().map(|notice| notice.name).collect()
}
