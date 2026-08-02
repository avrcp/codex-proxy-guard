use std::process::Command;

use proxy_guard_core::GuardConfig;
use proxy_guard_windows::{apply_proxy_environment, proxy_environment};

#[test]
fn child_inherits_scoped_proxy_environment_and_path() {
    let executable = env!("CARGO_BIN_EXE_child-env-probe");
    let environment = proxy_environment(&GuardConfig::default());
    let mut command = Command::new(executable);
    command.env("ALL_PROXY", "socks5://127.0.0.1:9999");
    apply_proxy_environment(&mut command, &environment);
    let output = command.output().expect("run env probe");
    assert!(output.status.success());
    let values: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(values["HTTP_PROXY"], environment.proxy_url);
    assert_eq!(values["HTTPS_PROXY"], environment.proxy_url);
    assert_eq!(values["NO_PROXY"], environment.no_proxy);
    assert!(values["ALL_PROXY"].is_null());
    assert!(values["PATH"].is_string());
}
