#define DOCTEST_CONFIG_IMPLEMENT
#ifdef _MSC_VER
#pragma warning(push, 0)
#endif
#include "doctest.h"
#ifdef _MSC_VER
#pragma warning(pop)
#endif

#include "cli_console.h"
#include "display_width.h"
#include "input.h"
#include "screen.h"
#include "session.h"
#include "tui_state.h"

#include <cwctype>

#include "../../tests/support/test_support.h"

using namespace cpg;
using namespace cpg::tui;

namespace {

Json snapshotFixture(bool canLaunch = true, bool enabled = false)
{
    return Json{
        {"config_readiness", "ready"},
        {"proxy", Json{{"url", "http://127.0.0.1:10808"}, {"host", "127.0.0.1"}, {"port", 10808}}},
        {"launch", Json{{"method", "appmodel_activation"}}},
        {"desktop", Json{{"state", "found"}, {"product", "chat_gpt"}, {"display_name", "ChatGPT Desktop"},
                         {"package_version", "26.1.2.3"}, {"architecture", "X64"},
                         {"application_id", "App"}, {"runtime_kind", "full_trust_desktop"}}},
        {"process", Json{{"state", "stopped"}, {"pid", nullptr}}},
        {"coverage", Json{{"state", enabled ? "current" : "not_authorized"}, {"enabled", enabled},
                          {"authorized_home", enabled ? Json("C:/fixture/.codex") : Json(nullptr)},
                          {"candidate_home", "C:/fixture/.codex"}}},
        {"busy", false}, {"elevation", "not_elevated"}, {"error", nullptr},
        {"actions", Json{{"can_launch", canLaunch}, {"can_edit_proxy", true},
                         {"can_authorize_backend_proxy", !enabled}, {"can_revoke_backend_proxy", enabled},
                         {"can_repair", canLaunch}, {"can_refresh", true}, {"can_cancel", false}}},
        {"confirmations", Json{
            {"backend_proxy", Json{{"token", "tok123"}, {"enabled", !enabled}, {"home", "C:/fixture/.codex"}}},
            {"repair", Json{{"token", "reptok"}}}}},
    };
}

Key key(wchar_t unicode)
{
    Key value;
    value.unicode = unicode;
    value.virtualKey = static_cast<WORD>(std::towlower(unicode) >= 'a' && std::towlower(unicode) <= 'z'
        ? std::towupper(unicode) : unicode);
    return value;
}

Key virtualKey(WORD code, wchar_t unicode = 0)
{
    Key value;
    value.virtualKey = code;
    value.unicode = unicode;
    return value;
}

} // namespace

TEST_CASE("display width covers ASCII, CJK, combining marks and surrogates")
{
    CHECK(displayWidth(L"abc") == 3);
    CHECK(displayWidth(L"\x4e2d\x6587") == 4);      // CJK ideographs are wide
    CHECK(displayWidth(L"a\x0301") == 1);           // combining acute is zero width
    CHECK(displayWidth(L"\xd83d\xde42") == 2);      // astral emoji measures as one wide cell
    CHECK(codeUnitWidth(L'a', false) == 1);
    CHECK(codeUnitWidth(L'\xdc00', true) == 0);     // low surrogate after a high one
    std::wstring truncated;
    appendTruncated(truncated, L"0123456789", 5);
    CHECK(truncated == L"01...");
    std::wstring wide;
    appendTruncated(wide, L"\x4e2d\x6587", 1);
    CHECK(wide.empty()); // a wide glyph cannot fit one cell
}

TEST_CASE("screen clips wide text and never writes out of bounds")
{
    Screen screen;
    screen.resize(10, 3);
    CHECK(screen.text(0, 0, L"0123456789ABCDEF") == 10);
    CHECK(screen.text(0, 1, L"\x4e2d\x6587\x4e2d\x6587\x4e2d\x6587") == 10); // 3 pairs = 6 cells? wide glyphs
    CHECK(screen.text(0, 5, L"outside") == 0); // row out of bounds
    CHECK(screen.text(100, 0, L"x") == 0);     // column out of bounds
    const std::wstring frame = renderFrame(screen);
    CHECK(frame.find(L"\x1b[H") == 0);
    CHECK(frame.find(L"\x1b[K") != std::wstring::npos);
    screen.resize(4, 2);
    CHECK(screen.width() == 4);
    CHECK(screen.height() == 2);
}

TEST_CASE("input normalization maps records to keys and resize")
{
    INPUT_RECORD records[4]{};
    records[0].EventType = KEY_EVENT;
    records[0].Event.KeyEvent.bKeyDown = TRUE;
    records[0].Event.KeyEvent.wVirtualKeyCode = 'E';
    records[0].Event.KeyEvent.uChar.UnicodeChar = L'e';
    records[1].EventType = KEY_EVENT; // key release: ignored
    records[1].Event.KeyEvent.bKeyDown = FALSE;
    records[2].EventType = WINDOW_BUFFER_SIZE_EVENT;
    records[3].EventType = MOUSE_EVENT;
    const InputBatch batch = normalizeRecords(records, 4);
    CHECK(batch.resized);
    REQUIRE(batch.keys.size() == 1);
    CHECK(batch.keys[0].unicode == L'e');
    CHECK(batch.keys[0].virtualKey == 'E');
}

TEST_CASE("key mapping follows the primary bindings")
{
    TuiState state;
    state.setSnapshot(snapshotFixture());
    CHECK(state.key(key(L'r')).kind == TuiState::Action::Kind::Refresh);
    CHECK(state.key(key(L'R')).kind == TuiState::Action::Kind::Refresh);
    CHECK(state.key(virtualKey(VK_F5)).kind == TuiState::Action::Kind::Refresh);
    CHECK(state.key(key(L'q')).kind == TuiState::Action::Kind::Quit);
    CHECK(state.key(virtualKey(VK_RETURN)).kind == TuiState::Action::Kind::Launch);
    CHECK(state.key(key(L'x')).kind == TuiState::Action::Kind::None);
    CHECK(state.key(virtualKey(VK_UP)).kind == TuiState::Action::Kind::None);
    // Modal openers seed the corresponding modal.
    state.key(key(L'e'));
    CHECK(state.modal() == Modal::ProxyEditor);
    state.key(virtualKey(VK_ESCAPE));
    CHECK(state.modal() == Modal::None);
    state.key(key(L'?'));
    CHECK(state.modal() == Modal::Help);
    state.key(key(L' '));
    CHECK(state.modal() == Modal::None);
}

TEST_CASE("the proxy editor edits fields like a bounded line editor")
{
    TuiState state;
    state.setSnapshot(snapshotFixture());
    state.setEditorSeed(L"127.0.0.1", L"10808");
    state.key(key(L'e'));
    REQUIRE(state.modal() == Modal::ProxyEditor);
    state.key(virtualKey(VK_TAB));           // port field
    state.key(virtualKey(VK_END));
    for (int index = 0; index < 5; ++index)
        state.key(virtualKey(VK_BACK));      // clear "10808"
    state.key(key(L'9'));
    const TuiState::Action saved = state.key(virtualKey(VK_RETURN));
    CHECK(saved.kind == TuiState::Action::Kind::SaveProxy);
    CHECK(saved.host == L"127.0.0.1");
    CHECK(saved.port == 9);
    // Port input rejects letters and caps length; host caps at 255.
    state.setEditorSeed(L"", L"");
    state.key(key(L'e'));
    state.key(virtualKey(VK_TAB));
    state.key(key(L'x'));
    state.key(virtualKey(VK_RETURN));
    CHECK_FALSE(state.editorError().empty());
}

TEST_CASE("YES confirmations are single use and token bound")
{
    TuiState state;
    state.setSnapshot(snapshotFixture());
    state.key(key(L'b'));
    REQUIRE(state.modal() == Modal::Consent);
    for (const wchar_t character : L"NO!")
        state.key(key(character));
    state.key(virtualKey(VK_RETURN));
    CHECK(state.modal() == Modal::Consent); // wrong word does not fire
    for (const wchar_t character : L"YES")
        state.key(key(character));
    const TuiState::Action consent = state.key(virtualKey(VK_RETURN));
    CHECK(consent.kind == TuiState::Action::Kind::ConfirmConsent);
    CHECK(consent.enableConsent);
    CHECK(consent.token == "tok123");
    CHECK(state.modal() == Modal::None);
    state.key(key(L'd'));
    REQUIRE(state.modal() == Modal::Repair);
    for (const wchar_t character : L"YES")
        state.key(key(character));
    const TuiState::Action repair = state.key(virtualKey(VK_RETURN));
    CHECK(repair.kind == TuiState::Action::Kind::ConfirmRepair);
    CHECK(repair.token == "reptok");
}

TEST_CASE("busy state gates destructive keys and keeps cancel paths")
{
    TuiState state;
    state.setSnapshot(snapshotFixture());
    state.setBusy(true, "Launching Desktop");
    CHECK(state.key(key(L'e')).kind == TuiState::Action::Kind::None);
    CHECK(state.key(key(L'b')).kind == TuiState::Action::Kind::None);
    CHECK(state.key(key(L'd')).kind == TuiState::Action::Kind::None);
    CHECK(state.key(virtualKey(VK_RETURN)).kind == TuiState::Action::Kind::None);
    Key cancel;
    cancel.control = true;
    cancel.unicode = 0x03;
    CHECK(state.key(cancel).kind == TuiState::Action::Kind::Cancel);
    state.key(key(L'q'));
    CHECK(state.modal() == Modal::QuitWhileBusy);
    CHECK(state.key(key(L'y')).kind == TuiState::Action::Kind::Cancel);
}

TEST_CASE("the blocked reason is stated in text, not only in color")
{
    Json snapshot = snapshotFixture(false);
    snapshot["process"] = Json{{"state", "running"}, {"pid", 12}};
    snapshot["error"] = Error("X", "y").json();
    const std::wstring reason = TuiState::blockedReason(snapshot);
    CHECK(reason.find(L"running") != std::wstring::npos);
    Json repair = snapshotFixture(false);
    repair["error"] = Error("CONFIG_INVALID", "broken").json();
    CHECK(TuiState::blockedReason(repair).find(L"E") != std::wstring::npos);
}

TEST_CASE("the error modal offers its suggested follow-up action")
{
    TuiState state;
    state.setSnapshot(snapshotFixture());
    Error failure("CODEX_ALREADY_RUNNING", "Desktop is running");
    state.setOperationOutcome(&failure);
    CHECK(state.modal() == Modal::ErrorResult);
    CHECK(state.key(key(L'r')).kind == TuiState::Action::Kind::Refresh);
    Error proxy("CONFIG_INVALID", "broken");
    state.setOperationOutcome(&proxy);
    CHECK(state.key(key(L'e')).kind == TuiState::Action::Kind::OpenEditor);
    Error backend("BACKEND_PROXY_REQUIRED_FOR_REPAIR", "authorize");
    state.setOperationOutcome(&backend);
    CHECK(state.key(key(L'c')).kind == TuiState::Action::Kind::CopyDiagnostic);
    CHECK(state.key(key(L'b')).kind == TuiState::Action::Kind::OpenConsent);
}

TEST_CASE("drawing a frame keeps every glyph inside the buffer")
{
    TuiState state;
    state.setSnapshot(snapshotFixture());
    state.setEditorSeed(L"127.0.0.1", L"10808");
    for (const int width : {72, 86, 40, 200}) {
        for (const int height : {22, 28, 10}) {
            Screen screen;
            screen.resize(width, height);
            state.draw(screen);
            const std::wstring frame = renderFrame(screen);
            CHECK(frame.size() > static_cast<std::size_t>(width));
        }
    }
    state.key(key(L'b'));
    Screen tiny;
    tiny.resize(30, 8);
    state.draw(tiny); // modal boxes clip on tiny frames without OOB
    CHECK(tiny.width() == 30);
}

TEST_CASE("the shared session keeps the single-foreground-operation gate")
{
    cpgtest::TemporaryDirectory root;
    const std::filesystem::path path = root.file(L"guard.toml");
    Config config;
    config.home = root.path;
    initializeConfig(path, config, false);
    Desktop noDesktopResult;
    EngineSession session(path,
        [](const Config &, const Cancellation &) -> Desktop {
            throw Error("APPX_NOT_FOUND", "No Desktop fixture.");
        },
        [] { return std::string("not_elevated"); });
    session.requestRefresh();
    for (int elapsed = 0; elapsed < 5000; ++elapsed) {
        if (session.poll() == EngineSession::Event::SnapshotArrived) break;
        Sleep(1);
    }
    Json snapshot = session.snapshot();
    CHECK(snapshot.value("config_readiness", "") == "ready");
    CHECK(snapshot.at("desktop").value("state", "") == "not_found");
    CHECK(snapshot.at("actions").value("can_launch", false) == false);
    std::string token = snapshot.at("confirmations").at("backend_proxy").value("token", "");
    CHECK_FALSE(token.empty());
    // A refused guess is single use: it burns the proposal, so a refresh must
    // precede the real confirmation.
    CHECK_THROWS_AS(session.requestSetBackendConsent(true, "guessed"), Error);
    session.requestRefresh();
    for (int elapsed = 0; elapsed < 5000; ++elapsed) {
        if (session.poll() == EngineSession::Event::SnapshotArrived) break;
        Sleep(1);
    }
    token = session.snapshot().at("confirmations").at("backend_proxy").value("token", "");
    // The typed path dispatches; a second concurrent request is refused.
    session.requestSetBackendConsent(true, token);
    CHECK_THROWS_AS(session.requestSetProxy("127.0.0.1", 7890), Error);
    for (int elapsed = 0; elapsed < 5000; ++elapsed) {
        if (!session.busy()) break;
        session.poll();
        Sleep(1);
    }
    snapshot = session.snapshot();
    CHECK(snapshot.at("coverage").value("enabled", false) == true);
    CHECK(Config::load(path).manageBackend);
    // Launches are never exercised here: the pipeline derives its activation
    // worker from the running executable, so a session test would perform a
    // real application-model activation. Launch semantics stay covered by the
    // engine fixture suites.
}

CPG_TEST_MAIN
