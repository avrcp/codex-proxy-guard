//! Port-chain acceptance for the proxy-port review (2026-10-27): the same
//! configuration file must drive the displayed proxy, the persisted TOML, the
//! launch plan and the consented Home block, with no stage silently reusing an
//! older value.
//!
//! Hermetic by construction: every test works in its own temp directory with
//! an injected discovery/activation seam. No real Desktop is activated, no
//! shared daemon is touched and no real Codex Home is written.

use cpg_domain::Error;
use cpg_domain::config::Config;
use cpg_domain::discovery::Desktop;
use cpg_domain::dotenv::{BLOCK_BEGIN, BLOCK_END, managed_block};
use cpg_domain::json::Json;
use cpg_domain::worker::empty_receipt;
use cpg_engine::config_store::{
    ConfigLease, initialize_config, inspect_proxy_env, load_config, prepare_proxy_env,
    revoke_proxy_env, update_proxy,
};
use cpg_engine::launch::{
    CodexCli, LaunchFacts, LaunchOptions, LaunchServices, proxy_environment, proxy_plan,
};
use cpg_engine::session::{EngineSession, SessionBudgets, SessionServices};
use cpg_windows::cancellation::Cancellation;
use cpg_windows::process_state::ProcessState;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(name: &str) -> PathBuf {
    let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
    let directory = std::env::temp_dir().join(format!(
        "cpg-chain-{}-{unique}-{}",
        std::process::id(),
        name
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("temp dir");
    directory
}

fn registered_desktop() -> Desktop {
    Desktop {
        registered: true,
        executable: PathBuf::from(r"C:\cpg-chain-fixture\app\ChatGPT.exe"),
        package_full_name: "OpenAI.Codex_26.1021.3705.0_x64__2p2nqsd0c76g0".to_owned(),
        package_family_name: "OpenAI.Codex_2p2nqsd0c76g0".to_owned(),
        application_id: "App".to_owned(),
        aumid: "OpenAI.Codex_2p2nqsd0c76g0!App".to_owned(),
        manifest_executable: "app/ChatGPT.exe".to_owned(),
        runtime_kind: "full_trust_desktop",
        ..Desktop::default()
    }
}

fn native_desktop() -> Desktop {
    Desktop {
        registered: false,
        executable: PathBuf::from(r"C:\cpg-chain-fixture\ChatGPT.exe"),
        runtime_kind: "full_trust_desktop",
        ..Desktop::default()
    }
}

fn settled(session: &mut EngineSession) {
    while session.busy() {
        session.poll();
        if session.busy() {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}

/// A session over a real configuration file with an injected Desktop and a
/// launch seam that is never driven by these tests.
fn session_with(path: &Path, desktop: Desktop) -> EngineSession {
    let services = SessionServices {
        process: |_| ProcessState {
            state: "stopped",
            pid: 0,
        },
        proposal_home: cpg_engine::config_store::consent_home,
        discover: Arc::new(move |_config, cancel| {
            cancel.check()?;
            Ok(desktop.clone())
        }),
        launch: Arc::new(|_config, _path, _options, _cancel, _facts| {
            Err(Error::new("UNUSED", "the launch seam is not driven here"))
        }),
    };
    EngineSession::with_services(
        path.to_path_buf(),
        services,
        SessionBudgets::default(),
        || "not_elevated",
    )
    .expect("session")
}

fn harness_session(path: &Path) -> EngineSession {
    session_with(path, registered_desktop())
}

// ---------------------------------------------------------------------------
// T01-T05: the saved port is the port every later stage reads
// ---------------------------------------------------------------------------

#[test]
fn edited_port_survives_the_file_reload_and_a_new_session() {
    let directory = temp_dir("roundtrip");
    let path = directory.join("config.toml");
    initialize_config(&path, &Config::default(), false).expect("seed default");

    // T01 the default configuration is the documented loopback endpoint.
    let initial = load_config(&path).expect("load initial");
    assert_eq!((initial.host.as_str(), initial.port), ("127.0.0.1", 10808));
    assert_eq!(initial.proxy_url(), "http://127.0.0.1:10808");

    // T02/T03 an editor-shaped mutation persists byte-for-byte.
    let updated = update_proxy(&path, &initial, false, "127.0.0.1", 10809).expect("update");
    assert_eq!(updated.port, 10809);
    let stored = std::fs::read_to_string(&path).expect("read stored");
    assert!(stored.contains("port = 10809"), "{stored}");
    assert!(!stored.contains("port = 10808"), "{stored}");

    // T04 a fresh process (a new session over the same path) reads back the
    // edited port and plans the edited endpoint, not the built-in default.
    let mut session = harness_session(&path);
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    assert!(view.ready);
    assert_eq!(view.proxy.port, 10809);
    assert_eq!(view.proxy.url, "http://127.0.0.1:10809");
    let reloaded = load_config(&path).expect("reload");
    assert_eq!(
        proxy_plan(&reloaded).expect("plan").endpoint,
        "http://127.0.0.1:10809"
    );

    // T05 a second, explicit configuration path is independent: editing one
    // file never changes what another session reads.
    let other = directory.join("elsewhere.toml");
    initialize_config(&other, &Config::default(), false).expect("seed other");
    assert_eq!(load_config(&other).expect("load other").port, 10808);
    update_proxy(&other, &Config::default(), false, "localhost", 7890).expect("update other");
    assert_eq!(load_config(&other).expect("reload other").port, 7890);
    assert_eq!(
        load_config(&path).expect("first file untouched").port,
        10809
    );
}

#[test]
fn failed_save_leaves_the_stored_configuration_untouched() {
    let directory = temp_dir("save-failure");
    let path = directory.join("config.toml");
    initialize_config(&path, &Config::default(), false).expect("seed");
    let before = std::fs::read(&path).expect("bytes before");
    let stored = load_config(&path).expect("load");

    // T06 an out-of-range port is refused by validation before any write.
    let error = update_proxy(&path, &stored, false, "127.0.0.1", 0).unwrap_err();
    assert_eq!(error.code, "CONFIG_INVALID");
    assert_eq!(std::fs::read(&path).expect("bytes after"), before);

    // An expectation that no longer matches the file is refused too.
    let mut drifted = stored.clone();
    drifted.port = 10999;
    let error = update_proxy(&path, &drifted, false, "127.0.0.1", 10809).unwrap_err();
    assert_eq!(error.code, "CONFIG_CHANGED");
    assert_eq!(load_config(&path).expect("still default").port, 10808);
}

#[test]
fn a_launch_refuses_a_configuration_that_changed_on_disk() {
    let directory = temp_dir("lease");
    let path = directory.join("config.toml");
    initialize_config(&path, &Config::default(), false).expect("seed");
    let snapshot = load_config(&path).expect("load");

    // The lease matches while nothing else touched the file.
    ConfigLease::new(&path, &snapshot).expect("matching lease");

    // T08 an external edit invalidates that very snapshot...
    update_proxy(&path, &snapshot, false, "127.0.0.1", 10809).expect("external edit");
    let error = match ConfigLease::new(&path, &snapshot) {
        Ok(_) => panic!("a snapshot that no longer matches the file must not lease"),
        Err(error) => error,
    };
    assert_eq!(error.code, "CONFIG_CHANGED");
    // ...and the refreshed snapshot leases cleanly again, so the retry uses
    // the new port rather than the stale one.
    let refreshed = load_config(&path).expect("reload");
    ConfigLease::new(&path, &refreshed).expect("refreshed lease");
    assert_eq!(
        proxy_plan(&refreshed).expect("plan").endpoint,
        "http://127.0.0.1:10809"
    );
}

// ---------------------------------------------------------------------------
// T09-T14, T16: the plan and the environment come from the passed-in config
// ---------------------------------------------------------------------------

#[test]
fn proxy_plan_tracks_the_configured_port_for_every_host_shape() {
    for (host, port, expected_url) in [
        ("127.0.0.1", 10808, "http://127.0.0.1:10808"),
        ("127.0.0.1", 10809, "http://127.0.0.1:10809"),
        ("localhost", 7890, "http://localhost:7890"),
        ("::1", 10809, "http://[::1]:10809"),
        ("127.0.0.53", 1, "http://127.0.0.53:1"),
    ] {
        let config = Config {
            host: host.to_owned(),
            port,
            ..Config::default()
        };
        let plan = proxy_plan(&config).unwrap_or_else(|error| panic!("{host}:{port} {error}"));
        assert_eq!(plan.endpoint, expected_url);
        assert!(
            plan.arguments
                .contains(&format!("--proxy-server=\"{expected_url}\"")),
            "{}",
            plan.arguments
        );
        // T12 the bypass list keeps the existing Chromium spellings.
        assert!(
            plan.arguments
                .contains("--proxy-bypass-list=\"localhost;127.0.0.1;[::1]\""),
            "{}",
            plan.arguments
        );
    }
}

#[test]
fn proxy_plan_rejects_what_the_configuration_may_still_store() {
    // A stored no_proxy entry can be schema-valid and still have no Chromium
    // bypass equivalent, so the launch plan is the first place that refuses it.
    let config = Config {
        no_proxy: vec!["localhost".to_owned(), "intranet.example".to_owned()],
        ..Config::default()
    };
    config.validate().expect("the schema accepts it");
    let error = proxy_plan(&config).unwrap_err();
    assert_eq!(error.code, "PROXY_BYPASS_UNSUPPORTED");

    // A no_proxy list is required, matching the stored configuration shape.
    let empty = Config {
        no_proxy: Vec::new(),
        ..Config::default()
    };
    assert_eq!(empty.validate().unwrap_err().code, "CONFIG_INVALID");
}

// ---------------------------------------------------------------------------
// T15: the view states the plan instead of letting the launch fail late
// ---------------------------------------------------------------------------

#[test]
fn the_view_states_the_plan_that_the_launch_will_actually_use() {
    let directory = temp_dir("plan-view");
    let path = directory.join("config.toml");
    initialize_config(
        &path,
        &Config {
            port: 10809,
            ..Config::default()
        },
        false,
    )
    .expect("seed");
    let mut session = harness_session(&path);
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    assert_eq!(view.proxy.state, "read");
    assert_eq!(view.proxy.url, "http://127.0.0.1:10809");
    assert_eq!(view.plan.state, "available");
    assert_eq!(view.plan.endpoint, "http://127.0.0.1:10809");
    assert!(view.plan.error.is_none());
    assert!(view.actions.can_launch);
}

#[test]
fn an_unbuildable_plan_blocks_a_registered_launch_before_any_side_effect() {
    let directory = temp_dir("plan-blocked");
    let path = directory.join("config.toml");
    initialize_config(
        &path,
        &Config {
            no_proxy: vec!["localhost".to_owned(), "intranet.example".to_owned()],
            ..Config::default()
        },
        false,
    )
    .expect("seed");
    let mut session = harness_session(&path);
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    assert!(view.ready, "the configuration itself read fine");
    assert_eq!(view.plan.state, "unavailable");
    assert!(view.plan.endpoint.is_empty(), "no endpoint is claimed");
    assert_eq!(
        view.plan
            .error
            .as_ref()
            .map(|error| error.code.as_str())
            .unwrap_or_default(),
        "PROXY_BYPASS_UNSUPPORTED"
    );
    assert_eq!(
        view.error.as_ref().map(|error| error.code.as_str()),
        Some("PROXY_BYPASS_UNSUPPORTED")
    );
    assert!(!view.actions.can_launch, "{}", view.actions.blocked_reason);

    // The refusal happens in the session, not deep inside the launch: the
    // harness launch seam answers UNUSED, so any other code proves nothing was
    // dispatched.
    let error = session.request_launch(false, "").unwrap_err();
    assert_eq!(error.code, "PROXY_BYPASS_UNSUPPORTED");
    settled(&mut session);
    assert!(session.launch_outcome().is_none(), "no launch was reported");
}

#[test]
fn a_native_target_still_launches_on_the_environment_channel() {
    // The bypass gap above only blocks the activation-argument channel. A
    // non-registered target receives the proxy through its process
    // environment, where an arbitrary NO_PROXY list is legitimate.
    let directory = temp_dir("plan-native");
    let path = directory.join("config.toml");
    initialize_config(
        &path,
        &Config {
            port: 10809,
            no_proxy: vec!["localhost".to_owned(), "intranet.example".to_owned()],
            ..Config::default()
        },
        false,
    )
    .expect("seed");
    let mut session = session_with(&path, native_desktop());
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    assert_eq!(view.launch_method, "native_process");
    assert_eq!(view.plan.state, "environment");
    assert_eq!(view.plan.endpoint, "http://127.0.0.1:10809");
    assert!(view.actions.can_launch);
    assert_eq!(view.coverage.state, "not_applicable");
}

#[test]
fn an_unread_configuration_claims_no_plan() {
    let directory = temp_dir("plan-unknown");
    let path = directory.join("config.toml");
    initialize_config(&path, &Config::default(), false).expect("seed");
    let mut session = harness_session(&path);
    session.request_refresh();
    settled(&mut session);
    assert_eq!(session.view().plan.state, "available");

    std::fs::write(&path, "broken toml [").expect("corrupt");
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    assert_eq!(view.proxy.state, "retained");
    assert_eq!(
        view.plan.state, "unknown",
        "a retained endpoint is displayed, never claimed as a launch plan"
    );
    assert!(!view.actions.can_launch);
}

#[test]
fn no_launch_path_reuses_another_configuration_than_the_one_it_was_given() {
    // T13/T14 the receipt endpoint is the plan for this launch.
    let directory = temp_dir("receipt");
    let path = directory.join("config.toml");
    let config = Config {
        port: 10809,
        ..Config::default()
    };
    initialize_config(&path, &config, false).expect("seed");

    let submitted: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let services = LaunchServices {
        config_path: &path,
        discover: &|_config, cancel| {
            cancel.check()?;
            Ok(registered_desktop())
        },
        prepare: &|_config| Ok(()),
        resolve: &|_config| {
            Ok(CodexCli {
                executable: PathBuf::from(r"C:\cpg-chain-fixture\codex.exe"),
                home: PathBuf::new(),
            })
        },
        stop: &|_cli, _cancel| Ok("stopped"),
        activate: &|_desktop, arguments, _cancel| {
            submitted.lock().expect("log").push(arguments.to_owned());
            let mut receipt = empty_receipt();
            receipt.insert("activation_returned", Json::Bool(true));
            receipt.insert("pid", Json::from(24681u64));
            receipt.insert("instance", Json::from("created"));
            receipt.insert("package_identity", Json::from("matched"));
            receipt.insert("aumid", Json::from("matched"));
            receipt.insert("elevation", Json::Bool(false));
            Ok(receipt)
        },
        mark_submitted: &|| Ok(()),
    };
    let mut facts = LaunchFacts::fresh();
    let receipt = cpg_engine::launch::launch_with(
        &config,
        LaunchOptions::default(),
        &Cancellation::new(),
        &services,
        &mut facts,
    )
    .expect("launch");

    assert_eq!(
        receipt.string_at("proxy_endpoint"),
        "http://127.0.0.1:10809"
    );
    let log = submitted.lock().expect("log");
    assert_eq!(log.len(), 1, "exactly one activation");
    assert!(log[0].contains("http://127.0.0.1:10809"), "{log:?}");
    assert!(!log[0].contains("10808"), "{log:?}");
    assert_eq!(receipt.string_at("instance"), "created");
    assert_eq!(
        receipt.string_at("proxy_delivery"),
        "activation_arguments",
        "no backend block was authorized"
    );
    assert_eq!(receipt.string_at("backend_proxy_config"), "not_authorized");
    drop(log);

    // T16 the environment channel for a plain EXE target follows the same port.
    let environment = proxy_environment(&config).expect("environment");
    for key in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
        assert_eq!(
            environment.value(key),
            Some("http://127.0.0.1:10809"),
            "{key} is stale"
        );
    }
    assert_eq!(
        environment.value("NO_PROXY"),
        Some("localhost,127.0.0.1,::1")
    );
    assert_eq!(
        environment.value("no_proxy"),
        Some("localhost,127.0.0.1,::1")
    );
    assert!(!environment.has("ALL_PROXY") && !environment.has("all_proxy"));
}

#[test]
fn a_cancel_before_the_submission_seam_says_nothing_was_submitted() {
    let directory = temp_dir("seam");
    let path = directory.join("config.toml");
    let config = Config {
        port: 10809,
        ..Config::default()
    };
    initialize_config(&path, &config, false).expect("seed");
    let activated = AtomicUsize::new(0);
    let services = LaunchServices {
        config_path: &path,
        discover: &|_config, cancel| {
            cancel.check()?;
            Ok(registered_desktop())
        },
        prepare: &|_config| Ok(()),
        resolve: &|_config| {
            Ok(CodexCli {
                executable: PathBuf::from(r"C:\cpg-chain-fixture\codex.exe"),
                home: PathBuf::new(),
            })
        },
        stop: &|_cli, _cancel| Ok("stopped"),
        activate: &|_desktop, _arguments, _cancel| {
            activated.fetch_add(1, Ordering::SeqCst);
            Err(Error::new("UNREACHED", "not activated in this test"))
        },
        mark_submitted: &|| Ok(()),
    };
    let cancel = Cancellation::new();
    cancel.cancel();
    let mut facts = LaunchFacts::fresh();
    let error = cpg_engine::launch::launch_with(
        &config,
        LaunchOptions::default(),
        &cancel,
        &services,
        &mut facts,
    )
    .unwrap_err();
    assert_eq!(error.code, "LAUNCH_CANCELLED");
    assert_eq!(facts.activation_state, "not_submitted");
    assert_eq!(activated.load(Ordering::SeqCst), 0, "no submission");
}

#[test]
fn a_cancel_after_the_submission_seam_is_never_relabeled_not_submitted() {
    // The activation call is where Desktop may already have been created, so a
    // cancellation landing inside it stays an unknown outcome.
    let directory = temp_dir("seam");
    let path = directory.join("config.toml");
    let config = Config {
        port: 10809,
        ..Config::default()
    };
    initialize_config(&path, &config, false).expect("seed");
    let activated = AtomicUsize::new(0);
    let services = LaunchServices {
        config_path: &path,
        discover: &|_config, cancel| {
            cancel.check()?;
            Ok(registered_desktop())
        },
        prepare: &|_config| Ok(()),
        resolve: &|_config| {
            Ok(CodexCli {
                executable: PathBuf::from(r"C:\cpg-chain-fixture\codex.exe"),
                home: PathBuf::new(),
            })
        },
        stop: &|_cli, _cancel| Ok("stopped"),
        activate: &|_desktop, _arguments, cancel| {
            activated.fetch_add(1, Ordering::SeqCst);
            cancel.cancel();
            cancel.check()?;
            Err(Error::new("UNREACHED", "not reached"))
        },
        mark_submitted: &|| Ok(()),
    };
    let cancel = Cancellation::new();
    let mut facts = LaunchFacts::fresh();
    let error = cpg_engine::launch::launch_with(
        &config,
        LaunchOptions::default(),
        &cancel,
        &services,
        &mut facts,
    )
    .unwrap_err();
    assert_eq!(error.code, "LAUNCH_CANCELLED");
    assert_eq!(
        facts.activation_state, "unknown",
        "past the seam Guard cannot prove the submission never happened"
    );
    assert_eq!(activated.load(Ordering::SeqCst), 1, "submission attempted");
}

// ---------------------------------------------------------------------------
// T21-T28: the consented Home block follows the same port
// ---------------------------------------------------------------------------

#[test]
fn home_block_is_absent_until_authorized_and_then_tracks_the_port() {
    let directory = temp_dir("env");
    let home = directory.join("codex-home");
    std::fs::create_dir_all(&home).expect("home");
    let env_path = home.join(".env");
    std::fs::write(&env_path, "USER_KEEP=1\r\n").expect("seed user bytes");

    // T21 unauthorized: neither inspect nor prepare may touch the file.
    let unconsented = Config {
        manage_backend: false,
        home: home.clone(),
        ..Config::default()
    };
    assert_eq!(inspect_proxy_env(&unconsented), "not_authorized");
    let error = prepare_proxy_env(&unconsented).unwrap_err();
    assert_eq!(error.code, "BACKEND_PROXY_SCOPE_UNCONFIRMED");
    assert_eq!(
        std::fs::read_to_string(&env_path).expect("user bytes"),
        "USER_KEEP=1\r\n"
    );

    let mut config = Config {
        manage_backend: true,
        home: home.clone(),
        ..Config::default()
    };

    // T22 authorized but not yet written: pending, then current after prepare.
    assert_eq!(inspect_proxy_env(&config), "pending");
    prepare_proxy_env(&config).expect("prepare");
    assert_eq!(inspect_proxy_env(&config), "current");
    let written = std::fs::read_to_string(&env_path).expect("written");
    assert!(written.starts_with("USER_KEEP=1\r\n"), "{written}");
    assert_eq!(
        written.matches(BLOCK_BEGIN).count(),
        1,
        "exactly one managed block: {written}"
    );

    // T23 the port moved: the block is stale and the next prepare rewrites it.
    config.port = 10809;
    assert_eq!(inspect_proxy_env(&config), "stale");
    prepare_proxy_env(&config).expect("re-prepare");
    assert_eq!(inspect_proxy_env(&config), "current");
    let updated = std::fs::read_to_string(&env_path).expect("updated");
    assert!(
        updated.contains("HTTP_PROXY=http://127.0.0.1:10809"),
        "{updated}"
    );
    assert!(!updated.contains("10808"), "{updated}");
    assert_eq!(updated.matches(BLOCK_BEGIN).count(), 1);
    assert_eq!(updated.matches(BLOCK_END).count(), 1);
    assert!(updated.starts_with("USER_KEEP=1\r\n"), "{updated}");

    // An idempotent prepare does not rewrite the file at all.
    let before = std::fs::read(&env_path).expect("bytes before");
    prepare_proxy_env(&config).expect("idempotent prepare");
    assert_eq!(std::fs::read(&env_path).expect("bytes after"), before);

    // T24 a key outside the block is a conflict Guard never merges away.
    std::fs::write(
        &env_path,
        format!("all_proxy=http://elsewhere\n{}", managed_block(&config)),
    )
    .expect("conflicting file");
    assert_eq!(inspect_proxy_env(&config), "conflict");
    let error = prepare_proxy_env(&config).unwrap_err();
    assert_eq!(error.code, "BACKEND_PROXY_CONFIG_CONFLICT");
    assert!(
        std::fs::read_to_string(&env_path)
            .expect("conflict untouched")
            .starts_with("all_proxy=http://elsewhere"),
        "refusal must not rewrite the file"
    );

    // T25 a manually damaged block is refused, never silently replaced.
    let damaged = format!("{BLOCK_BEGIN}\nHTTP_PROXY=x\n{BLOCK_END}\n");
    std::fs::write(&env_path, &damaged).expect("damaged");
    assert_eq!(inspect_proxy_env(&config), "invalid");
    let error = prepare_proxy_env(&config).unwrap_err();
    assert_eq!(error.code, "BACKEND_PROXY_BLOCK_INVALID");
    assert_eq!(
        std::fs::read_to_string(&env_path).expect("bytes kept"),
        damaged
    );

    // T27 an unsupported multi-line syntax refuses the whole file.
    let unsupported = "NOTE=\"two\nlines\"\n";
    std::fs::write(&env_path, unsupported).expect("multiline");
    assert_eq!(inspect_proxy_env(&config), "unsupported");
    let error = prepare_proxy_env(&config).unwrap_err();
    assert_eq!(error.code, "BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED");
    assert_eq!(
        std::fs::read_to_string(&env_path).expect("bytes kept"),
        unsupported
    );

    // T28 revoking removes only Guard's own block and keeps everything else.
    let layout = format!("A=1\n{}\nB=2\n", managed_block(&config));
    std::fs::write(&env_path, &layout).expect("block layout");
    revoke_proxy_env(&home).expect("revoke");
    assert_eq!(
        std::fs::read_to_string(&env_path).expect("revoked"),
        "A=1\n\nB=2\n"
    );
    assert_eq!(
        inspect_proxy_env(&config),
        "pending",
        "block gone, consent kept"
    );
}

#[test]
fn an_unbound_home_is_refused_without_creating_anything() {
    let directory = temp_dir("unbound-home");
    let path = directory.join("config.toml");
    let config = Config {
        manage_backend: true,
        home: PathBuf::from(".\\guessed\\.codex"),
        ..Config::default()
    };
    initialize_config(&path, &config, false).expect("seed");
    assert_eq!(inspect_proxy_env(&config), "invalid");
    let error = prepare_proxy_env(&config).unwrap_err();
    assert_eq!(error.code, "BACKEND_PROXY_SCOPE_UNCONFIRMED");
    assert!(
        !directory.join("guessed").exists(),
        "a relative Home must never be created from the current directory"
    );
}

// ---------------------------------------------------------------------------
// Regressions: a refresh that never read the configuration must not roll the
// displayed state back to the built-in defaults.
// ---------------------------------------------------------------------------

#[test]
fn a_failed_refresh_retains_the_last_known_good_configuration() {
    let directory = temp_dir("retained");
    let path = directory.join("config.toml");
    let config = Config {
        port: 10809,
        ..Config::default()
    };
    initialize_config(&path, &config, false).expect("seed 10809");
    let mut session = harness_session(&path);
    session.request_refresh();
    settled(&mut session);
    assert_eq!(session.view().proxy.port, 10809);

    // Another program breaks the file. Nothing about the stored port changed,
    // so the displayed port must not revert to the built-in default.
    std::fs::write(&path, "broken toml [").expect("corrupt");
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    assert!(
        view.error.is_some(),
        "the unreadable file must still be reported"
    );
    assert_eq!(
        view.proxy.port, 10809,
        "a read failure must not reset the displayed proxy to the defaults"
    );
    assert_eq!(view.proxy.url, "http://127.0.0.1:10809");
    assert!(
        !view.actions.can_launch,
        "launch stays gated until a refresh actually succeeded"
    );

    // Saving over a file Guard can read but not parse keeps the last known
    // good fields instead of collapsing them back to the defaults.
    session
        .request_set_proxy("127.0.0.1", 10810)
        .expect("save accepted");
    settled(&mut session);
    let outcome = session.mutation_outcome().expect("mutation outcome");
    assert!(outcome.committed, "{outcome:?}");
    let recovered = load_config(&path).expect("recovered config");
    assert_eq!(recovered.port, 10810);
    assert_eq!(recovered.host, "127.0.0.1");
    assert_eq!(
        recovered.no_proxy,
        vec!["localhost", "127.0.0.1", "::1"],
        "the retained snapshot keeps the unrelated fields"
    );

    // A good read always wins over the retained snapshot.
    let restored = Config {
        port: 10807,
        ..Config::default()
    };
    std::fs::write(&path, restored.to_toml().expect("toml")).expect("restore");
    session.request_refresh();
    settled(&mut session);
    assert_eq!(session.view().proxy.port, 10807);
}

#[test]
fn a_failed_read_does_not_offer_an_unsafe_recovery() {
    let directory = temp_dir("unreadable");
    let path = directory.join("config.toml");
    let config = Config {
        port: 10809,
        ..Config::default()
    };
    initialize_config(&path, &config, false).expect("seed 10809");
    let mut session = harness_session(&path);
    session.request_refresh();
    settled(&mut session);

    // A directory in place of the configuration is a read failure, not
    // evidence that the stored bytes are invalid. Guard must not overwrite it
    // with a recovered configuration just because the read failed.
    std::fs::remove_file(&path).expect("remove file");
    std::fs::create_dir_all(&path).expect("directory occupies the path");
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    assert_eq!(view.proxy.port, 10809, "the snapshot is still displayed");
    assert!(view.error.is_some(), "and the failure is reported");
    session
        .request_set_proxy("127.0.0.1", 10810)
        .expect("save accepted");
    settled(&mut session);
    let outcome = session.mutation_outcome().expect("mutation outcome");
    assert!(
        !outcome.committed,
        "a path Guard cannot read must not be written over"
    );
    assert_eq!(
        outcome
            .mutation_error
            .as_ref()
            .map(|error| error.code.as_str())
            .unwrap_or_default(),
        "CONFIG_LOCK_FAILED",
        "the refusal names the path problem, not a phantom conflict"
    );
    assert!(
        path.is_dir(),
        "nothing may occupy the path Guard could not read"
    );
    assert!(
        !directory.join("config.toml.toml").exists(),
        "recovery must not invent a sibling file"
    );
}

#[test]
fn a_committed_save_survives_a_failing_following_refresh() {
    let directory = temp_dir("committed");
    let path = directory.join("config.toml");
    initialize_config(&path, &Config::default(), false).expect("seed");
    let mut session = harness_session(&path);
    session.request_refresh();
    settled(&mut session);
    // T07 save succeeds, then the file becomes unreadable for the follow-up
    // refresh: the committed fact must not be displayed as the old port.
    session
        .request_set_proxy("127.0.0.1", 10809)
        .expect("save accepted");
    settled(&mut session);
    let outcome = session.mutation_outcome().expect("mutation outcome");
    assert!(outcome.committed);
    assert!(outcome.refresh_error.is_none());
    assert_eq!(session.view().proxy.port, 10809);

    std::fs::write(&path, "broken toml [").expect("corrupt");
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    assert_eq!(view.proxy.port, 10809, "the committed value survives");
    assert!(view.error.is_some(), "and the read failure is still shown");
}

// ---------------------------------------------------------------------------
// Review #2: which refusal is described, and what a failed read may still claim
// ---------------------------------------------------------------------------

#[test]
fn an_unusable_configuration_is_offered_as_a_repair_not_a_refresh() {
    // Guard read the bytes and proved they cannot be used. Telling the user to
    // refresh describes the one action that cannot help, and this string is
    // also what the CONFIG_INVALID error carries to every frontend.
    let directory = temp_dir("invalid-wording");
    let path = directory.join("config.toml");
    initialize_config(&path, &Config::default(), false).expect("seed");
    let mut session = harness_session(&path);
    session.request_refresh();
    settled(&mut session);

    std::fs::write(&path, "broken toml [").expect("corrupt");
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    assert_eq!(
        view.error.as_ref().map(|error| error.code.as_str()),
        Some("CONFIG_INVALID")
    );
    assert_eq!(
        view.actions.blocked_reason,
        "The stored configuration is not usable; set the proxy again to repair it."
    );
    assert!(
        view.actions.can_edit_proxy,
        "the offered action is the save"
    );

    // A read that never produced bytes, after Guard had read the file once:
    // refresh is the action and no recovery write is authorized.
    std::fs::remove_file(&path).expect("remove");
    std::fs::create_dir_all(&path).expect("directory");
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    assert_eq!(view.proxy.state, "retained");
    assert_eq!(
        view.actions.blocked_reason,
        "Guard could not read the configuration; refresh before launching."
    );

    // Before any read at all, neither of those sentences applies.
    let unreadable = directory.join("not-a-file");
    std::fs::create_dir_all(&unreadable).expect("directory");
    let mut session = harness_session(&unreadable);
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    assert_eq!(
        view.actions.blocked_reason,
        "Guard has not read a usable configuration yet; refresh before launching."
    );
}

#[test]
fn a_failed_read_stops_claiming_the_backend_authorization() {
    let directory = temp_dir("coverage-stale");
    let home = directory.join("codex-home");
    std::fs::create_dir_all(&home).expect("home");
    let path = directory.join("config.toml");
    initialize_config(
        &path,
        &Config {
            manage_backend: true,
            home: home.clone(),
            ..Config::default()
        },
        false,
    )
    .expect("seed");
    let mut session = harness_session(&path);
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    assert!(view.coverage.enabled, "authorized while the file reads");
    assert_eq!(
        view.coverage.authorized_home.as_deref(),
        Some(home.as_path())
    );

    std::fs::write(&path, "broken toml [").expect("corrupt");
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    // The endpoint is still shown for reading convenience, marked retained…
    assert_eq!(view.proxy.state, "retained");
    // …but consent is a fact about the file, and the file was not understood.
    assert!(
        !view.coverage.enabled,
        "a stale authorization is not presented as current"
    );
    assert_eq!(view.coverage.authorized_home, None);
    assert!(!view.actions.can_launch, "{}", view.actions.blocked_reason);
}

#[test]
fn a_view_that_has_read_nothing_states_no_empty_state() {
    // Every state string is documented; a default view must not invent a fifth
    // empty one that the frontends render as a blank cell.
    let view = cpg_engine::session::SessionView::default();
    assert_eq!(view.proxy.state, "unconfirmed");
    assert_eq!(view.plan.state, "unknown");
    let json = view.public_json();
    assert_eq!(json.at("proxy").at("source").as_str(), Some("unconfirmed"));
    assert_eq!(
        json.at("launch_plan").at("state").as_str(),
        Some("unknown"),
        "the public JSON keeps the documented state"
    );
}

#[test]
fn a_recreated_configuration_does_not_restore_backend_consent() {
    // Recovery from a proven-invalid file rewrites the whole file. If the file
    // has since disappeared, recreating it with the consent fields would
    // re-authorize a Home without any prompt.
    let directory = temp_dir("recreate-consent");
    let home = directory.join("codex-home");
    std::fs::create_dir_all(&home).expect("home");
    let path = directory.join("config.toml");
    let authorized = Config {
        manage_backend: true,
        home: home.clone(),
        ..Config::default()
    };
    initialize_config(&path, &authorized, false).expect("seed");
    let expected = load_config(&path).expect("read");
    std::fs::remove_file(&path).expect("remove");

    let error = update_proxy(&path, &expected, true, "127.0.0.1", 10809).unwrap_err();
    assert_eq!(error.code, "CONFIG_CONSENT_REPAIR_REQUIRED");
    assert!(
        !path.exists(),
        "the refused repair does not create the file it would have written"
    );

    // Without consent fields, recreating a missing configuration is a repair.
    let plain_path = directory.join("plain.toml");
    let plain = Config {
        port: 10808,
        ..Config::default()
    };
    initialize_config(&plain_path, &plain, false).expect("seed");
    std::fs::remove_file(&plain_path).expect("remove");
    let saved = update_proxy(&plain_path, &plain, true, "127.0.0.1", 10809).expect("repair");
    assert_eq!(saved.port, 10809);
    assert!(!saved.manage_backend);
}

#[test]
fn a_session_that_never_read_a_file_shows_no_endpoint() {
    // The built-in default endpoint on screen reads as "my port got reset",
    // which is the very confusion this review started from. Before any
    // successful read Guard has no endpoint to show.
    let directory = temp_dir("no-default-display");
    let unreadable = directory.join("not-a-file");
    std::fs::create_dir_all(&unreadable).expect("directory");
    let mut session = harness_session(&unreadable);
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    assert_eq!(view.proxy.state, "unconfirmed");
    assert!(
        view.proxy.url.is_empty() && view.proxy.host.is_empty() && view.proxy.port == 0,
        "no endpoint is invented for a file Guard never read: {:?}",
        view.proxy
    );
    let refusal = session
        .request_launch(false, "")
        .expect_err("a session that read nothing cannot launch");
    assert!(
        refusal.message.contains("Refresh"),
        "a file Guard could not read is repaired by refreshing: {}",
        refusal.message
    );

    // A real read restores the endpoint without needing a default.
    let path = directory.join("config.toml");
    initialize_config(
        &path,
        &Config {
            port: 10809,
            ..Config::default()
        },
        false,
    )
    .expect("seed");
    let mut session = harness_session(&path);
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    assert_eq!(view.proxy.url, "http://127.0.0.1:10809");
    assert_eq!(view.proxy.state, "read");
}

#[test]
fn an_unusable_stored_file_states_itself_instead_of_a_missing_read() {
    // "not read yet" would send the user to refresh, and a refresh reports the
    // same unusable bytes. Guard read the file and proved it cannot be used,
    // so the published state says exactly that and offers the repair.
    let directory = temp_dir("invalid-source");
    let path = directory.join("config.toml");
    std::fs::write(&path, b"this is not the Guard configuration\n").expect("write");
    let mut session = harness_session(&path);
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    assert_eq!(
        view.error.as_ref().map(|error| error.code.as_str()),
        Some("CONFIG_INVALID")
    );
    assert_eq!(view.proxy.state, "invalid");
    assert!(
        !view.proxy.endpoint_known && view.proxy.url.is_empty(),
        "bytes Guard could not parse publish no endpoint: {:?}",
        view.proxy
    );
    assert!(
        view.actions.blocked_reason.contains("not usable"),
        "{}",
        view.actions.blocked_reason
    );
    let refusal = session
        .request_launch(false, "")
        .expect_err("unusable stored bytes cannot launch");
    assert_eq!(refusal.code, "CONFIG_INVALID");
    assert_eq!(refusal.message, view.actions.blocked_reason);

    // A read that succeeded earlier outranks the later failure: the last good
    // endpoint stays on screen, labelled as the last good read.
    let path = directory.join("kept.toml");
    initialize_config(
        &path,
        &Config {
            port: 10809,
            ..Config::default()
        },
        false,
    )
    .expect("seed");
    let mut session = harness_session(&path);
    session.request_refresh();
    settled(&mut session);
    assert_eq!(session.view().proxy.state, "read");
    std::fs::write(&path, b"this is not the Guard configuration\n").expect("corrupt");
    session.request_refresh();
    settled(&mut session);
    let view = session.view();
    assert_eq!(view.proxy.state, "retained");
    assert!(view.proxy.endpoint_known);
    assert_eq!(view.proxy.url, "http://127.0.0.1:10809");
    assert_eq!(
        view.error.as_ref().map(|error| error.code.as_str()),
        Some("CONFIG_INVALID")
    );
}
