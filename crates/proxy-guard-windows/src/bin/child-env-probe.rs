use std::collections::BTreeMap;
use std::path::PathBuf;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentPackageFullName(length: *mut u32, name: *mut u16) -> i32;
}

#[cfg(windows)]
fn package_full_name() -> Result<Option<String>, i32> {
    let mut length = 0_u32;
    let status = unsafe { GetCurrentPackageFullName(&mut length, std::ptr::null_mut()) };
    if status == 15700 {
        return Ok(None);
    }
    if status != 122 || !(2..=1024).contains(&length) {
        return Err(status);
    }
    let mut buffer = vec![0_u16; length as usize];
    let status = unsafe { GetCurrentPackageFullName(&mut length, buffer.as_mut_ptr()) };
    if status != 0 {
        return Err(status);
    }
    let end = buffer.iter().position(|unit| *unit == 0).ok_or(87)?;
    String::from_utf16(&buffer[..end]).map(Some).map_err(|_| 87)
}

fn main() {
    let names = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "http_proxy",
        "https_proxy",
        "NO_PROXY",
        "no_proxy",
        "ALL_PROXY",
        "all_proxy",
        "PATH",
    ];
    let mut values = names
        .into_iter()
        .map(|name| (name, std::env::var(name).ok()))
        .collect::<BTreeMap<_, _>>();
    #[cfg(windows)]
    values.insert("PACKAGE_FULL_NAME", package_full_name().ok().flatten());
    let output = serde_json::to_string(&values).expect("serialize env");
    let mut arguments = std::env::args_os().skip(1);
    match (arguments.next(), arguments.next(), arguments.next()) {
        (None, None, None) => println!("{output}"),
        (Some(flag), Some(path), None) if flag == "--output" => {
            std::fs::write(PathBuf::from(path), output).expect("write probe output");
        }
        (Some(flag), Some(path), None) if flag == "--spawn-child" => {
            let path = PathBuf::from(path);
            std::fs::write(path.with_extension("parent.json"), output)
                .expect("write parent probe output");
            let status = std::process::Command::new(std::env::current_exe().expect("self path"))
                .arg("--output")
                .arg(path)
                .env("HTTP_PROXY", "http://127.0.0.1:18999")
                .env("HTTPS_PROXY", "http://127.0.0.1:18999")
                .env("NO_PROXY", "localhost")
                .env_remove("ALL_PROXY")
                .status()
                .expect("spawn child probe");
            assert!(status.success(), "child probe failed: {status}");
        }
        _ => panic!("usage: child-env-probe [--output PATH | --spawn-child PATH]"),
    }
}
