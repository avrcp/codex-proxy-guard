//! Plain line-mode console: the VT-unavailable / `--plain` / redirected
//! fallback. Same engine semantics, line-driven interaction, no ANSI.

use cpg_domain::text::public_text;
use cpg_domain::{Error, Result};
use cpg_engine::{EngineSession, MutationOutcome};
use std::io::{BufRead, Write};
use std::path::Path;

fn prompt(text: &str) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = writeln!(out, "{text}");
    let _ = out.flush();
}

/// Reads one trimmed line from stdin; `None` on EOF.
fn line(input: &mut dyn BufRead) -> Option<String> {
    let mut buffer = String::new();
    match input.read_line(&mut buffer) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(buffer.trim().to_owned()),
    }
}

fn wait_with_controls(
    session: &mut EngineSession,
    controls: &cpg_windows::console::ConsoleControlScope,
) {
    while session.busy() {
        if controls.take_cancel() {
            session.cancel();
        }
        if controls.take_close() {
            session.cancel();
        }
        session.poll();
        if session.busy() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

pub fn run(config_path: &Path) -> Result<i32> {
    let controls = cpg_windows::console::ConsoleControlScope::new();
    let mut session = EngineSession::new(config_path.to_path_buf())?;
    // Even on an input/output exception, reap the operation before
    // acknowledging close.
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let result = interact(&mut session, &mut input, &controls);
    session.cancel();
    while session.busy() {
        let _ = session.poll();
        if session.busy() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    result
}

fn interact(
    session: &mut EngineSession,
    input: &mut dyn BufRead,
    controls: &cpg_windows::console::ConsoleControlScope,
) -> Result<i32> {
    prompt(&format!(
        "Codex Proxy Guard {} - local proxy launcher",
        crate::terminal::diagnostic_version()
    ));
    session.request_refresh();
    wait_with_controls(session, controls);
    let mut editor_host = String::new();
    let mut editor_port = String::new();
    // Retained outcomes are printed exactly once per operation id; without
    // this guard every later command would re-print the last launch receipt
    // or mutation failure forever.
    let mut reported_launch: Option<i64> = None;
    let mut reported_mutation: Option<i64> = None;
    loop {
        let view = session.view();
        let configured = if view.proxy.state == "read" {
            view.proxy.url.clone()
        } else {
            format!("{} [{}]", view.proxy.url, view.proxy.state)
        };
        prompt(&format!(
            "Proxy: {} | Plan: {}{} | Process: {} | Coverage: {}",
            configured,
            view.plan.state,
            if view.plan.endpoint.is_empty() {
                String::new()
            } else {
                format!(" {}", view.plan.endpoint)
            },
            view.process.state,
            view.coverage.state
        ));
        if let Some(error) = &view.error {
            prompt(&format!("{}: {}", error.code, error.message));
        }
        let Some(action) = line(input) else {
            return Ok(0);
        };
        if controls.take_close() {
            return Ok(0);
        }
        controls.take_cancel(); // idle Ctrl+C is a no-op, preserving proposals
        if action.is_empty() {
            continue;
        }
        if action.len() != 1 {
            prompt("Choose one listed command.");
            continue;
        }
        let command = action.to_ascii_lowercase().chars().next().unwrap();
        match command {
            'q' => return Ok(0),
            'r' => {
                session.request_refresh();
                wait_with_controls(session, controls);
            }
            'l' => {
                if let Err(error) = session.request_launch(false, "") {
                    prompt(&format!("{}: {}", error.code, error.message));
                }
                wait_with_controls(session, controls);
            }
            'e' => {
                let view = session.view();
                prompt(&format!(
                    "Loopback HTTP/Mixed host [{}] (Enter keeps draft):",
                    if editor_host.is_empty() {
                        view.proxy.host.clone()
                    } else {
                        editor_host.clone()
                    }
                ));
                let Some(host) = line(input) else {
                    return Ok(0);
                };
                prompt(&format!(
                    "Port [{}] (Enter keeps draft):",
                    if editor_port.is_empty() {
                        view.proxy.port.to_string()
                    } else {
                        editor_port.clone()
                    }
                ));
                let Some(port) = line(input) else {
                    return Ok(0);
                };
                let view = session.view();
                editor_host = if host.is_empty() {
                    view.proxy.host.clone()
                } else {
                    host
                };
                editor_port = if port.is_empty() {
                    view.proxy.port.to_string()
                } else {
                    port
                };
                let parsed = cpg_domain::parse_port_strict(&editor_port);
                match parsed {
                    Ok(port) => {
                        if let Err(error) = session.request_set_proxy(&editor_host, port as i64) {
                            prompt(&format!("{}: {}", error.code, error.message));
                        }
                        wait_with_controls(session, controls);
                    }
                    Err(error) => {
                        prompt(&format!("{}: {}", error.code, error.message));
                    }
                }
            }
            'b' | 'd' => {
                if command == 'b' {
                    if let Some(consent) = session.view().consent.clone() {
                        prompt(&format!(
                            "{}\n{}",
                            if consent.enabled {
                                "Authorize Guard proxy block for later Codex processes in:"
                            } else {
                                "Remove Guard's unchanged proxy block from:"
                            },
                            consent.home.display()
                        ));
                    } else {
                        prompt("Backend authorization unavailable - press R to refresh.");
                        continue;
                    }
                } else if session.view().repair.is_some() {
                    prompt(
                        "Repair stops the shared Codex daemon once and may interrupt other clients' tasks on the same Home.",
                    );
                } else {
                    prompt("Repair launch unavailable - press R to refresh.");
                    continue;
                }
                prompt("Type YES to confirm this single operation (anything else cancels):");
                let Some(confirm) = line(input) else {
                    return Ok(0);
                };
                if confirm == "YES" {
                    // Route through the same session confirmation path.
                    if command == 'd' {
                        if let Some(repair) = session.view().repair.clone()
                            && let Err(error) = session.request_launch(true, &repair.token)
                        {
                            prompt(&format!("{}: {}", error.code, error.message));
                        }
                    } else if let Some(consent) = session.view().consent.clone()
                        && let Err(error) = session.request_set_backend_consent(&consent)
                    {
                        prompt(&format!("{}: {}", error.code, error.message));
                    }
                    wait_with_controls(session, controls);
                }
            }
            _ => prompt("Choose one listed command."),
        }
        // Publish results once per operation id; a retained outcome would
        // otherwise be re-printed after every later command.
        if let Some(outcome) = session.launch_outcome()
            && reported_launch != Some(outcome.operation_id)
        {
            reported_launch = Some(outcome.operation_id);
            if let Some(error) = &outcome.error {
                prompt(&format!("{}: {}", error.code, error.message));
            } else if let Some(receipt) = &outcome.receipt {
                prompt(&format!(
                    "Launched: method={} PID={} instance={}",
                    receipt.launch_method,
                    receipt
                        .pid
                        .map(|pid| pid.to_string())
                        .unwrap_or_else(|| "unknown".into()),
                    receipt.instance
                ));
                prompt(&format!(
                    "Proxy delivery: {} endpoint={}",
                    receipt.proxy_delivery,
                    receipt.proxy_endpoint.as_deref().unwrap_or("-")
                ));
                prompt(&format!(
                    "Backend config: {} daemon={}",
                    receipt.backend_proxy_config, receipt.daemon_preparation
                ));
            }
            prompt("Delivery facts only: Desktop adoption and network use are not verified.");
        }
        if let Some(MutationOutcome {
            operation_id,
            mutation_error: Some(error),
            ..
        }) = session.mutation_outcome()
            && reported_mutation != Some(operation_id)
        {
            reported_mutation = Some(operation_id);
            prompt(&format!(
                "{}: {}",
                error.code,
                public_text(&error.message, 1024)
            ));
        }
    }
}

#[allow(dead_code)]
fn sink(error: Option<Error>) {
    let _ = error;
}
