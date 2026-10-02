//! Exercise the actual process boundary, including shutdown while the parent
//! still holds stdin. Targets always point at a nonexistent test executable;
//! these tests cannot activate Desktop or stop a shared daemon.

use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

use proxy_guard_core::GuardConfig;
use serde_json::{Value, json};

struct Engine {
    child: Child,
    stdin: Option<ChildStdin>,
    output: Receiver<String>,
    root: PathBuf,
}

impl Engine {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("cpg-bridge-test-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let mut config = GuardConfig::default();
        config.codex.executable_override = root.join("never-created.exe");
        config.codex.proxy_env_home = root.join("codex-home");
        config.save(&root.join("guard.toml")).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_codex-proxy-guard"))
            .arg("--config")
            .arg(root.join("guard.toml"))
            .arg("bridge")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            output,
            root,
        }
    }

    fn send(&mut self, id: u64, method: &str, params: Value) {
        writeln!(
            self.stdin.as_mut().unwrap(),
            "{}",
            json!({"schema":1,"id":id,"method":method,"params":params})
        )
        .unwrap();
    }

    fn read(&self) -> Value {
        let line = self
            .output
            .recv_timeout(Duration::from_secs(10))
            .expect("engine response deadline");
        assert!(line.len() <= 128 * 1024);
        let value: Value =
            serde_json::from_str(&line).expect("stdout must contain only protocol JSON");
        assert_eq!(value["schema"], 1);
        value
    }

    fn hello(&mut self) {
        self.send(1, "hello", json!({}));
        assert_eq!(self.read()["result"]["protocol_version"], 1);
    }

    fn exited(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            assert!(
                Instant::now() < deadline,
                "bridge did not exit within its cleanup budget"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Only this test's own bridge child is terminated on a failed test.
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(self.root.join("guard.toml"));
        let _ = std::fs::remove_dir(self.root.join("codex-home"));
        let _ = std::fs::remove_dir(&self.root);
    }
}

#[test]
fn hello_and_shutdown_exit_while_parent_holds_stdin_open() {
    let mut engine = Engine::new();
    engine.hello();
    engine.send(2, "shutdown", json!({}));
    assert_eq!(engine.read()["result"]["shutting_down"], true);
    assert!(engine.stdin.is_some());
    engine.exited();
}

#[test]
fn strict_decode_errors_and_oversize_are_protocol_only() {
    let mut engine = Engine::new();
    engine.hello();
    engine.stdin.as_mut().unwrap().write_all(b"\xff\n").unwrap();
    assert_eq!(engine.read()["error"]["code"], "BRIDGE_INVALID_UTF8");
    engine.stdin.as_mut().unwrap().write_all(b"{}{}\n").unwrap();
    assert_eq!(engine.read()["error"]["code"], "BRIDGE_INVALID_JSON");
    engine
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"{\"schema\":2,\"id\":2,\"method\":\"hello\",\"params\":{}}\n")
        .unwrap();
    assert_eq!(engine.read()["error"]["code"], "BRIDGE_SCHEMA_UNSUPPORTED");
    engine.send(3, "execute_command", json!({}));
    assert_eq!(engine.read()["error"]["code"], "BRIDGE_METHOD_UNSUPPORTED");
    let _ = engine
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&vec![b'x'; 32 * 1024 + 1]);
    assert_eq!(engine.read()["error"]["code"], "BRIDGE_REQUEST_TOO_LARGE");
    engine.exited();
}

#[test]
fn proxy_and_consent_use_real_dispatcher_without_touching_home_content() {
    let mut engine = Engine::new();
    engine.hello();
    let before = std::fs::read(engine.root.join("guard.toml")).unwrap();
    engine.send(2, "set_proxy", json!({"host":"8.8.8.8","port":7890}));
    assert_eq!(engine.read()["error"]["code"], "CONFIG_INVALID");
    assert_eq!(
        std::fs::read(engine.root.join("guard.toml")).unwrap(),
        before
    );
    engine.send(3, "set_proxy", json!({"host":"127.0.0.1","port":7890}));
    let snapshot = engine.read()["result"].clone();
    assert_eq!(snapshot["proxy"]["port"], 7890);
    assert_eq!(
        snapshot["desktop"]["state"], "not_found",
        "mutation response must await actual discovery/coverage refresh"
    );
    if snapshot["elevation"] == "not_elevated" {
        assert_eq!(snapshot["error"]["code"], "CODEX_EXECUTABLE_MISSING");
    }
    assert_eq!(snapshot["coverage"]["enabled"], false);
    let token = snapshot["confirmations"]["backend_proxy"]["token"]
        .as_str()
        .unwrap()
        .to_owned();
    engine.send(
        4,
        "set_backend_proxy_consent",
        json!({"enabled":true,"confirmation_token":token}),
    );
    assert_eq!(engine.read()["result"]["coverage"]["enabled"], true);
    let config = GuardConfig::load(&engine.root.join("guard.toml")).unwrap();
    assert!(config.codex.manage_codex_proxy_env);
    assert_eq!(config.codex.proxy_env_home, engine.root.join("codex-home"));
    assert!(
        !engine.root.join("codex-home").join(".env").exists(),
        "granting consent must not prepare the block"
    );
    engine.send(
        5,
        "set_backend_proxy_consent",
        json!({"enabled":false,"confirmation_token":token}),
    );
    assert_eq!(engine.read()["error"]["code"], "CONFIRMATION_REQUIRED");
    assert!(
        GuardConfig::load(&engine.root.join("guard.toml"))
            .unwrap()
            .codex
            .manage_codex_proxy_env
    );
    engine.send(6, "shutdown", json!({}));
    assert_eq!(engine.read()["ok"], true);
    engine.exited();
}

#[test]
fn changed_config_invalidates_consent_scope_and_eof_exits() {
    let mut engine = Engine::new();
    engine.hello();
    engine.send(2, "snapshot", json!({}));
    let snapshot = engine.read()["result"].clone();
    assert_eq!(snapshot["desktop"]["state"], "not_found");
    assert!(!snapshot.to_string().contains("never-created.exe"));
    let token = snapshot["confirmations"]["backend_proxy"]["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut config = GuardConfig::load(&engine.root.join("guard.toml")).unwrap();
    config.codex.proxy_env_home = engine.root.join("different-home");
    config.save(&engine.root.join("guard.toml")).unwrap();
    engine.send(
        3,
        "set_backend_proxy_consent",
        json!({"enabled":true,"confirmation_token":token}),
    );
    assert_eq!(engine.read()["error"]["code"], "CONFIG_CHANGED");
    assert!(
        !GuardConfig::load(&engine.root.join("guard.toml"))
            .unwrap()
            .codex
            .manage_codex_proxy_env
    );
    // A partial launch is discarded at EOF rather than submitted.
    engine
        .stdin
        .as_mut()
        .unwrap()
        .write_all(
            b"{\"schema\":1,\"id\":4,\"method\":\"start_launch\",\"params\":{\"repair\":false}}",
        )
        .unwrap();
    engine.stdin.take();
    engine.exited();
    assert!(engine.output.try_recv().is_err());
}

#[test]
fn launch_ack_precedes_events_and_normal_launch_reaches_a_terminal_result() {
    let mut engine = Engine::new();
    engine.hello();
    engine.send(2, "start_launch", json!({"repair":false}));
    let ack = engine.read();
    // Elevated test hosts fail closed before dispatch; do not weaken that gate.
    if ack["ok"] == false {
        assert_eq!(ack["error"]["code"], "ELEVATED_LAUNCH_UNSUPPORTED");
    } else {
        assert_eq!(ack["id"], 2);
        assert_eq!(ack["result"]["operation_id"], 1);
        assert_eq!(engine.read()["event"], "operation_state");
        let terminal = engine.read();
        assert_eq!(terminal["event"], "operation_finished");
        assert_eq!(terminal["operation_id"], 1);
        assert_eq!(
            terminal["ok"], false,
            "nonexistent executable cannot launch"
        );
    }
    engine.send(3, "shutdown", json!({}));
    assert_eq!(engine.read()["ok"], true);
    engine.exited();
}
