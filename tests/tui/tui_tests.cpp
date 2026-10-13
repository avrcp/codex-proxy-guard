#define DOCTEST_CONFIG_IMPLEMENT
#ifdef _MSC_VER
#pragma warning(push, 0)
#endif
#include "doctest.h"
#ifdef _MSC_VER
#pragma warning(pop)
#endif

#include "cli_console.h"
#include "clipboard.h"
#include "display_width.h"
#include "input.h"
#include "screen.h"
#include "session.h"
#include "tui_state.h"
#include "tui_controller.h"
#include "terminal_session.h"
#include "plain_input.h"
#include "interactive_terminal_scope.h"
#include "cpg/text.h"
#include "cpg/win_handle.h"
#include "cpg/console_control.h"
#include <sstream>
#include <thread>

#include <atomic>
#include <cwctype>

#include "../../tests/support/test_support.h"

using namespace cpg;
using namespace cpg::tui;

namespace {

SessionView snapshotFixture(bool canLaunch = true, bool enabled = false)
{
    SessionView view;
    view.ready = true;
    view.proxy = {"http://127.0.0.1:10808", "127.0.0.1", 10808};
    view.launchMethod = "appmodel_activation";
    view.desktop.state = "found"; view.desktop.product = "chat_gpt";
    view.desktop.displayName = "ChatGPT Desktop"; view.desktop.packageVersion = "26.1.2.3";
    view.desktop.architecture = "X64"; view.desktop.applicationId = "App";
    view.desktop.runtimeKind = "full_trust_desktop";
    view.process.state = "stopped";
    view.coverage.state = enabled ? "current" : "not_authorized";
    view.coverage.enabled = enabled;
    if (enabled) view.coverage.authorizedHome = L"C:/fixture/.codex";
    view.coverage.candidateHome = L"C:/fixture/.codex";
    view.actions.canLaunch = canLaunch; view.actions.canEditProxy = true;
    view.actions.canAuthorizeBackendProxy = !enabled; view.actions.canRevokeBackendProxy = enabled;
    view.actions.canRepair = canLaunch; view.actions.canRefresh = true;
    view.actions.blockedReason = "Desktop unavailable - press R to refresh.";
    view.consent = ConsentProposal{"tok123", !enabled, L"C:/fixture/.codex", 1};
    view.repair = RepairProposal{"reptok", 1};
    return view;
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

TEST_CASE("screen writes an emoji atomically at the right boundary")
{
    Screen screen;
    screen.resize(2, 1);
    const wchar_t emoji[] = L"\xd83d\xde42";
    CHECK(screen.text(0, 0, emoji) == 2);
    CHECK(screen.cell(0, 0).glyph == emoji);
    CHECK(screen.cell(1, 0).continuation);
    const std::wstring frame = renderFrame(screen);
    CHECK(frame.find(emoji) != std::wstring::npos);
    // The pair is emitted exactly once (no duplicated low half via the
    // continuation cell).
    const std::size_t pairPos = frame.find(emoji);
    CHECK(frame.find(emoji, pairPos + 1) == std::wstring::npos);
}

TEST_CASE("emoji that cannot fit is not partially written")
{
    Screen screen;
    screen.resize(1, 1);
    const wchar_t emoji[] = L"\xd83d\xde42";
    CHECK(screen.text(0, 0, emoji) == 0);
    const std::wstring frame = renderFrame(screen);
    CHECK(frame.find(emoji) == std::wstring::npos);
    CHECK(frame.find(L"\xd83d", 3) == std::wstring::npos); // no stray high half
    CHECK(screen.cell(0, 0).glyph == L" ");
}

TEST_CASE("combining mark attaches to the previous glyph")
{
    Screen screen;
    screen.resize(4, 1);
    CHECK(screen.text(0, 0, L"a\x0301") == 1);
    CHECK(screen.cell(0, 0).glyph == L"a\x0301");
    CHECK_FALSE(screen.cell(1, 0).continuation);
    CHECK(renderFrame(screen).find(L"a\x0301") != std::wstring::npos);
    // A mark after a wide glyph attaches to that glyph's leading cell.
    CHECK(screen.text(1, 0, L"\x4e2d\x0301") == 2);
    CHECK(screen.cell(1, 0).glyph == L"\x4e2d\x0301");
    CHECK(screen.cell(2, 0).continuation);
}

TEST_CASE("zero-width cluster at the clipped boundary never writes out of bounds")
{
    Screen screen;
    screen.resize(1, 1);
    CHECK(screen.text(0, 0, L"a") == 1);
    // The mark lands on the existing cell, not on cell(1, 0).
    CHECK(screen.text(1, 0, L"\x0301") == 0);
    CHECK(screen.cell(0, 0).glyph == L"a\x0301");
    // A mark with no preceding cell in the row is dropped.
    Screen fresh;
    fresh.resize(4, 1);
    CHECK(fresh.text(0, 0, L"\x0301" L"b") == 1);
    CHECK(fresh.cell(0, 0).glyph == L"b");
    // Out-of-frame writes never attach to anything.
    CHECK(screen.text(100, 0, L"\x0301") == 0);
}

TEST_CASE("isolated surrogate becomes replacement character")
{
    const DisplayCluster high = nextDisplayCluster(L"\xd83d", 0);
    CHECK(high.text == L"\xfffd");
    CHECK(high.width == 1);
    CHECK(high.consumed == 1);
    const DisplayCluster low = nextDisplayCluster(L"a\xdc00", 1);
    CHECK(low.text == L"\xfffd");
    CHECK(low.width == 1);
    Screen screen;
    screen.resize(2, 1);
    CHECK(screen.text(0, 0, L"\xd83d") == 1);
    CHECK(screen.cell(0, 0).glyph == L"\xfffd");
    const std::wstring frame = renderFrame(screen);
    CHECK(frame.find(L"\xd83d") == std::wstring::npos);
    CHECK(frame.find(L"\xfffd") != std::wstring::npos);
}

TEST_CASE("wide clusters clip exactly at the frame edge")
{
    Screen screen;
    screen.resize(3, 1);
    // Only the first CJK glyph fits; the second must not be partially written.
    CHECK(screen.text(0, 0, L"\x4e2d\x6587") == 2);
    CHECK(screen.cell(0, 0).glyph == L"\x4e2d");
    CHECK(screen.cell(1, 0).continuation);
    CHECK(screen.cell(2, 0).glyph == L" ");
    Screen mixed;
    mixed.resize(4, 1);
    // Emoji (2) + ASCII (1) leaves 1: the trailing CJK glyph does not fit.
    CHECK(mixed.text(0, 0, L"\xd83d\xde42" L"a\x4e2d") == 3);
    CHECK(mixed.cell(3, 0).glyph == L" ");
}

TEST_CASE("cluster-aware truncation never splits a glyph")
{
    std::wstring out;
    appendTruncated(out, L"\xd83d\xde42", 1);
    CHECK(out.empty()); // a wide cluster cannot fit one cell, not even half of it
    out.clear();
    CHECK(appendTruncated(out, L"a\xd83d\xde42" L"bcd", 4) == 4);
    CHECK(out == L"a...");
    out.clear();
    // Combining marks travel with their base, and an isolated surrogate is
    // normalized instead of being copied through.
    appendTruncated(out, L"a\x0301\xd83d" L"b", 20);
    CHECK(out == L"a\x0301\xfffd" L"b");
    out.clear();
    // "C:/☺🙂/.ćodex" into 5 cells: the emoji cannot be half-kept, so the
    // ellipsis rolls back past ☺ and only "C:..." fits.
    const std::wstring emojiHome = L"C:/\x263a\xd83d\xde42/.c\x0301odex";
    CHECK(appendTruncated(out, emojiHome, 5) == 5);
    CHECK(out == L"C:...");
}

TEST_CASE("the main view renders unicode homes without escaping the frame")
{
    TuiState state;
    SessionView snapshot = snapshotFixture(true, true);
    snapshot.coverage.authorizedHome = utf8ToWide("C:/users/\xf0\x9f\x99\x82\xc3\xa1/.c\xc2\xa1odex");
    state.setSnapshot(snapshot);
    for (const int width : {72, 40, 30}) {
        Screen screen;
        screen.resize(width, 22);
        REQUIRE_NOTHROW(state.draw(screen));
        const std::wstring frame = renderFrame(screen);
        // Every high surrogate in the frame is part of a complete pair —
        // truncation of the home path never emits a dangling half.
        std::size_t at = frame.find(L"\xd83d");
        while (at != std::wstring::npos) {
            CHECK(frame.find(L"\xd83d\xde42", at) == at);
            at = frame.find(L"\xd83d", at + 2);
        }
        if (width >= 40) // the emoji still fits; a narrow frame cuts before it
            CHECK(frame.find(L"\xd83d\xde42") != std::wstring::npos);
    }
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
    // L launches too — the documented "Enter / L" shortcut is really bound.
    CHECK(state.key(key(L'l')).kind == TuiState::Action::Kind::Launch);
    CHECK(state.key(key(L'L')).kind == TuiState::Action::Kind::Launch);
    CHECK(state.key(key(L'x')).kind == TuiState::Action::Kind::None);
    CHECK(state.key(virtualKey(VK_UP)).kind == TuiState::Action::Kind::None);
    // Modal openers seed the corresponding modal.
    state.key(key(L'e'));
    CHECK(state.modal() == Modal::ProxyEditor);
    state.key(virtualKey(VK_ESCAPE));
    CHECK(state.modal() == Modal::None);
    state.key(key(L'?'));
    CHECK(state.modal() == Modal::Help);
    state.key(virtualKey(VK_ESCAPE));
    CHECK(state.modal() == Modal::None);
}

TEST_CASE("modal openers respect the snapshot capability flags")
{
    TuiState state;
    SessionView snapshot = snapshotFixture(false);
    snapshot.actions = Capabilities{};
    snapshot.actions.canRefresh = true;
    state.setSnapshot(snapshot);
    CHECK(state.key(key(L'e')).kind == TuiState::Action::Kind::None);
    CHECK(state.key(key(L'b')).kind == TuiState::Action::Kind::None);
    CHECK(state.key(key(L'd')).kind == TuiState::Action::Kind::None);
    CHECK(state.modal() == Modal::None); // no modal opens without capability
    Screen screen;
    screen.resize(80, 24);
    state.draw(screen);
    const std::wstring frame = renderFrame(screen);
    // The status line carries the most recent blocked-opener notice as text.
    CHECK(frame.find(L"Repair launch unavailable") != std::wstring::npos);
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
    CHECK(state.key(key(L'y')).kind == TuiState::Action::Kind::CancelAndQuit);
}

TEST_CASE("the blocked reason is stated in text, not only in color")
{
    SessionView snapshot = snapshotFixture(false);
    snapshot.process = {"running", 12};
    snapshot.actions.blockedReason = "Desktop is running - fully exit it before launching.";
    snapshot.error = Error("X", "y");
    const std::wstring reason = TuiState::blockedReason(snapshot);
    CHECK(reason.find(L"running") != std::wstring::npos);
    SessionView repair = snapshotFixture(false);
    repair.error = Error("CONFIG_INVALID", "broken");
    repair.actions.blockedReason = "Proxy configuration needs repair - press E to edit it.";
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

TEST_CASE("the TUI renders before the first session snapshot arrives")
{
    // Regression: the initial empty snapshot must render (missing keys never
    // reach a null receiver, which would throw json type_error.306).
    TuiState state;
    Screen screen;
    screen.resize(80, 24);
    REQUIRE_NOTHROW(state.draw(screen));
    const std::wstring frame = renderFrame(screen);
    CHECK(frame.find(L"CODEX PROXY GUARD") != std::wstring::npos);
    CHECK_FALSE(TuiState::blockedReason(SessionView{}).empty());
    // Capability-gated keys fail closed before the first snapshot arrives.
    CHECK(state.key(key(L'e')).kind == TuiState::Action::Kind::None);
    CHECK(state.key(key(L'b')).kind == TuiState::Action::Kind::None);
    CHECK(state.key(key(L'd')).kind == TuiState::Action::Kind::None);
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

TEST_CASE("timed-out operations stay busy until their future is reaped")
{
    cpgtest::TemporaryDirectory root;
    const std::filesystem::path path = root.file(L"guard.toml");
    Config config;
    config.home = root.path;
    initializeConfig(path, config, false);

    std::atomic<int> active{0};
    std::atomic<int> maxActive{0};
    const auto slowDiscovery = [&](const Config &, const Cancellation &cancel) -> Desktop {
        int now = active.fetch_add(1) + 1;
        int seen = maxActive.load();
        while (now > seen && !maxActive.compare_exchange_weak(seen, now)) {}
        for (int elapsed = 0; elapsed < 500 && !cancel.cancelled(); ++elapsed) Sleep(1);
        Sleep(30); // bounded cleanup that continues after cancellation
        active.fetch_sub(1);
        throw Error("APPX_NOT_FOUND", "No Desktop fixture.");
    };
    SessionBudgets budgets;
    budgets.mutation = std::chrono::milliseconds(20);
    // Destruction of a session with a live timed-out effect cancels it and
    // reaps the future before returning (covered by this scope).
    EngineSession session(path, slowDiscovery, [] { return std::string("not_elevated"); },
                          budgets);
    session.requestRefresh();
    bool cancelling = false;
    for (int elapsed = 0; elapsed < 2000 && !cancelling; ++elapsed) {
        session.poll();
        cancelling = session.cancellingAfterTimeout();
        if (!cancelling) Sleep(1);
    }
    REQUIRE(cancelling);
    // The old future is still running: the session must stay busy and refuse
    // a second foreground operation instead of overlapping it.
    CHECK(session.busy());
    CHECK_THROWS_AS(session.requestSetProxy("127.0.0.1", 7890), Error);
    EngineSession::Event event = EngineSession::Event::None;
    for (int elapsed = 0; elapsed < 5000; ++elapsed) {
        event = session.poll();
        if (!session.busy()) break;
        Sleep(1);
    }
    CHECK(event == EngineSession::Event::MutationFailed);
    CHECK_FALSE(session.busy());
    CHECK(session.mutationError().has_value());
    CHECK(session.mutationError()->code == "SESSION_TIMEOUT");
    CHECK(maxActive.load() == 1); // the refused request never started
    // After the reap the gate is open again for a new operation.
    session.requestRefresh();
    CHECK(session.busy());
}

namespace {

struct RecordingClipboardApi : public ClipboardApi {
    bool failOpen = false, failEmpty = false, failLock = false, failUnlock = false,
         failSet = false;
    bool opened = false, emptied = false, closed = false, setTextCalled = false;
    int allocations = 0, frees = 0;
    void *lastFreed = nullptr;
    wchar_t buffer[512] = {};

    bool open(void *) override { opened = true; return !failOpen; }
    bool empty() override { emptied = true; return !failEmpty; }
    void *allocate(std::size_t) override
    {
        ++allocations;
        return reinterpret_cast<void *>(0x1234);
    }
    void *lock(void *) override
    {
        if (failLock) return nullptr;
        return buffer;
    }
    bool unlock(void *) override { return !failUnlock; }
    bool setText(void *) override
    {
        setTextCalled = true;
        return !failSet;
    }
    void freeMemory(void *memory) override { ++frees; lastFreed = memory; }
    void close() override { closed = true; }
};

} // namespace

TEST_CASE("clipboard copy obeys Win32 handle ownership rules")
{
    RecordingClipboardApi api;
    // Success: the accepted HGLOBAL is never freed; the text arrived intact.
    CHECK(copyDiagnosticText(L"diagnostic 中文", api, reinterpret_cast<void *>(7)));
    CHECK(api.opened);
    CHECK(api.emptied);
    CHECK(api.setTextCalled);
    CHECK(api.closed);
    CHECK(api.allocations == 1);
    CHECK(api.frees == 0);
    CHECK(api.buffer == std::wstring(L"diagnostic 中文"));
    // SetClipboardData refused: ownership never transferred, so it is freed.
    RecordingClipboardApi refused;
    refused.failSet = true;
    CHECK_FALSE(copyDiagnosticText(L"x", refused, nullptr));
    CHECK(refused.frees == 1);
    CHECK(refused.lastFreed == reinterpret_cast<void *>(0x1234));
    // Earlier failures never leak an allocation either.
    RecordingClipboardApi emptyFailed;
    emptyFailed.failEmpty = true;
    CHECK_FALSE(copyDiagnosticText(L"x", emptyFailed, nullptr));
    CHECK(emptyFailed.allocations == 0);
    RecordingClipboardApi lockFailed;
    lockFailed.failLock = true;
    CHECK_FALSE(copyDiagnosticText(L"x", lockFailed, nullptr));
    CHECK(lockFailed.frees == 1);
    RecordingClipboardApi unlockFailed;
    unlockFailed.failUnlock = true;
    CHECK_FALSE(copyDiagnosticText(L"x", unlockFailed, nullptr));
    CHECK(unlockFailed.frees == 1);
    RecordingClipboardApi openFailed;
    openFailed.failOpen = true;
    CHECK_FALSE(copyDiagnosticText(L"x", openFailed, nullptr));
    CHECK(openFailed.allocations == 0);
}

TEST_CASE("the copy notice is transient feedback on the status line")
{
    TuiState state;
    state.setSnapshot(snapshotFixture());
    state.setNotice("Diagnostic copied");
    Screen screen;
    screen.resize(80, 24);
    state.draw(screen);
    std::wstring frame = renderFrame(screen);
    CHECK(frame.find(L"Diagnostic copied") != std::wstring::npos);
    // The next snapshot replaces it with the usual status line.
    state.setSnapshot(snapshotFixture());
    state.draw(screen);
    frame = renderFrame(screen);
    CHECK(frame.find(L"Diagnostic copied") == std::wstring::npos);
}

TEST_CASE("RC3 Enter on Host submits the complete proxy form")
{
    TuiState state;
    state.setSnapshot(snapshotFixture());
    state.key(key(L'e'));
    state.setEditorSeed(L"127.0.0.1", L"7890");
    const auto action = state.key(virtualKey(VK_RETURN));
    CHECK(action.kind == TuiState::Action::Kind::SaveProxy);
    CHECK(action.port == 7890);
}

TEST_CASE("RC3 a pending save retains its editor and draft")
{
    TuiState state;
    state.setSnapshot(snapshotFixture());
    state.key(key(L'e'));
    state.setEditorSeed(L"localhost", L"7890");
    state.setBusy(true, "Saving");
    CHECK(state.modal() == Modal::ProxyEditor);
}

TEST_CASE("RC3 nullable unpackaged entry fields render without exceptions")
{
    TuiState state;
    auto snapshot = snapshotFixture();
    snapshot.desktop.applicationId.reset();
    snapshot.desktop.runtimeKind.reset();
    state.setSnapshot(snapshot);
    Screen screen;
    screen.resize(80, 24);
    CHECK_NOTHROW(state.draw(screen));
}

TEST_CASE("RC3 result refresh works and help ignores unrelated keys")
{
    TuiState state;
    state.setSnapshot(snapshotFixture());
    state.setOperationOutcome(nullptr);
    CHECK(state.key(key(L'r')).kind == TuiState::Action::Kind::Refresh);
    state.clearModal();
    state.key(key(L'?'));
    state.key(key(L'x'));
    CHECK(state.modal() == Modal::Help);
}


namespace {
void pump(TuiController &controller)
{
    for (int count = 0; controller.busy() && count < 5000; ++count) { controller.poll(); Sleep(1); }
    REQUIRE_FALSE(controller.busy());
}
Desktop absentDesktop(const Config &, const Cancellation &) { throw Error("APPX_NOT_FOUND", "No fixture Desktop."); }
std::string normalUser() { return "not_elevated"; }
void yes(TuiController &controller)
{
    for (const wchar_t ch : std::wstring_view(L"YES")) controller.handleKey(key(ch));
    controller.handleKey(virtualKey(VK_RETURN));
}
struct FakeTerminal : TerminalApi {
    DWORD inputMode = ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_QUICK_EDIT_MODE | ENABLE_VIRTUAL_TERMINAL_INPUT;
    DWORD outputMode = 0;
    CONSOLE_CURSOR_INFO cursor{23, FALSE};
    int getModeCalls = 0, setModeCalls = 0, cursorWrites = 0, writes = 0;
    int failGetMode = 0, failSetMode = 0;
    bool failGetCursor = false, failCursorWrite = false, failAlt = false;
    HANDLE standardHandle(DWORD id) override { return reinterpret_cast<HANDLE>(static_cast<std::uintptr_t>(id == STD_INPUT_HANDLE ? 1 : 2)); }
    bool getMode(HANDLE h, DWORD &mode) override {
        if (++getModeCalls == failGetMode) return false;
        mode = h == reinterpret_cast<HANDLE>(1) ? inputMode : outputMode; return true;
    }
    bool setMode(HANDLE h, DWORD mode) override {
        if (++setModeCalls == failSetMode) return false;
        (h == reinterpret_cast<HANDLE>(1) ? inputMode : outputMode) = mode; return true;
    }
    bool getCursor(HANDLE, CONSOLE_CURSOR_INFO &value) override { value = cursor; return !failGetCursor; }
    bool setCursor(HANDLE, const CONSOLE_CURSOR_INFO &value) override {
        ++cursorWrites; if (failCursorWrite) return false; cursor = value; return true;
    }
    bool write(HANDLE, std::wstring_view value) override { ++writes; return !(failAlt && value == L"\x1b[?1049h"); }
};
}
TEST_CASE("RC3 production handler opens first consent with the exact engine proposal")
{
    cpgtest::TemporaryDirectory root;
    Config config; config.home = root.path;
    const auto path = root.file(L"guard.toml"); initializeConfig(path, config, false);
    EngineSession session(path, absentDesktop, normalUser);
    TuiState state; TuiController controller(session, state);
    controller.refresh(); pump(controller);
    REQUIRE(session.view().consent.has_value());
    const auto proposal = *session.view().consent;
    CHECK_FALSE(session.view().coverage.authorizedHome.has_value());
    CHECK_NOTHROW(controller.handleKey(key(L'b')));
    REQUIRE(state.modal() == Modal::Consent);
    REQUIRE(state.consentProposal().has_value());
    CHECK(*state.consentProposal() == proposal);
    Screen screen; screen.resize(86, 28); CHECK_NOTHROW(state.draw(screen));
    controller.handleKey(virtualKey(VK_ESCAPE));
    CHECK_FALSE(Config::load(path).manageBackend);
    controller.handleKey(key(L'b')); yes(controller); pump(controller);
    CHECK(Config::load(path).manageBackend);
    CHECK(Config::load(path).home == proposal.home);
}
TEST_CASE("RC3 displayed consent is frozen and stale configuration is refused")
{
    cpgtest::TemporaryDirectory root;
    Config config; config.home = root.path;
    const auto path = root.file(L"guard.toml"); initializeConfig(path, config, false);
    EngineSession session(path, absentDesktop, normalUser);
    TuiState state; TuiController controller(session, state);
    controller.refresh(); pump(controller); controller.handleKey(key(L'b'));
    const auto proposal = *state.consentProposal();
    auto newer = session.view(); newer.consent->token = "different"; newer.consent->home = root.file(L"other");
    state.setSnapshot(newer);
    CHECK(*state.consentProposal() == proposal);
    updateProxy(path, config, false, "localhost", 7888);
    yes(controller); pump(controller);
    CHECK_FALSE(Config::load(path).manageBackend);
    CHECK(Config::load(path).port == 7888);
    CHECK(state.modal() == Modal::ErrorResult);
    CHECK_FALSE(state.snapshot().consent.has_value());
    controller.handleKey(key(L'b'));
    CHECK(state.modal() == Modal::ErrorResult);
    controller.handleKey(key(L'd'));
    CHECK(state.modal() == Modal::ErrorResult);
}
TEST_CASE("RC3 missing proposal blocks B and real unpackaged session renders")
{
    cpgtest::TemporaryDirectory root;
    Config config; config.home = root.path;
    const auto path = root.file(L"guard.toml"); initializeConfig(path, config, false);
    SessionServices services;
    services.proposalHome = [](const Config &) { return std::filesystem::path{}; };
    services.process = [](const Desktop &) { ProcessState result; result.state = "stopped"; return result; };
    EngineSession session(path, [](const Config &, const Cancellation &) { Desktop result; result.displayName = "Fixture Desktop"; return result; }, normalUser, {}, services);
    TuiState state; TuiController controller(session, state);
    controller.refresh(); pump(controller);
    CHECK_FALSE(session.view().desktop.applicationId.has_value());
    Screen screen; screen.resize(72, 22); CHECK_NOTHROW(state.draw(screen));
    controller.handleKey(key(L'b')); CHECK(state.modal() == Modal::None);
    CHECK_FALSE(Config::load(path).manageBackend);
}
TEST_CASE("RC3 production save keeps draft on failure and separates committed refresh failure")
{
    cpgtest::TemporaryDirectory root;
    Config config; config.home = root.path;
    const auto path = root.file(L"guard.toml"); initializeConfig(path, config, false);
    EngineSession session(path, absentDesktop, normalUser);
    TuiState state; TuiController controller(session, state);
    controller.refresh(); pump(controller); controller.handleKey(key(L'e'));
    state.setEditorSeed(L"localhost", L"7890");
    const HANDLE lock = CreateFileW(path.c_str(), GENERIC_READ, 0, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    REQUIRE(lock != INVALID_HANDLE_VALUE);
    controller.handleKey(virtualKey(VK_RETURN));
    controller.handleKey(virtualKey(VK_RETURN)); // same InputBatch cannot submit twice
    CHECK(state.modal() == Modal::ProxyEditor);
    pump(controller); CloseHandle(lock);
    CHECK(state.editorHost() == L"localhost"); CHECK(state.editorPort() == L"7890");
    CHECK_FALSE(state.editorError().empty());
    controller.handleKey(virtualKey(VK_RETURN)); pump(controller);
    CHECK(Config::load(path).port == 7890);
    REQUIRE(session.mutationOutcome()); CHECK(session.mutationOutcome()->committed);
    CHECK(session.mutationOutcome()->refreshError.has_value());
    CHECK(state.modal() == Modal::None);
    MutationOutcome late; late.operationId = session.operationId() - 1;
    late.mutationError = Error("LATE", "Old save failed");
    state.applyMutationOutcome(late); CHECK(state.modal() == Modal::None);
}
TEST_CASE("RC3 production busy quit defaults to keep and exits only after reap")
{
    cpgtest::TemporaryDirectory root;
    Config config; config.home = root.path;
    const auto path = root.file(L"guard.toml"); initializeConfig(path, config, false);
    std::atomic_bool entered{false};
    EngineSession session(path, [&](const Config &, const Cancellation &cancel) -> Desktop {
        entered = true; while (!cancel.cancelled()) Sleep(1); Sleep(15); cancel.check(); return {};
    }, normalUser);
    TuiState state; TuiController controller(session, state); controller.refresh();
    for (int count = 0; !entered && count < 2000; ++count) Sleep(1);
    controller.handleKey(key(L'q')); controller.handleKey(virtualKey(VK_RETURN));
    CHECK_FALSE(controller.exitRequested()); CHECK(controller.busy());
    controller.handleKey(key(L'q')); controller.handleKey(key(L'y'));
    CHECK(controller.exitRequested()); CHECK_FALSE(controller.shouldExit());
    pump(controller); CHECK(controller.shouldExit());
}
TEST_CASE("RC3 launch result persists across one automatic refresh and warns on reuse")
{
    cpgtest::TemporaryDirectory root;
    Config config; config.home = root.path;
    const auto path = root.file(L"guard.toml"); initializeConfig(path, config, false);
    std::atomic_int discoveries{0}, launches{0};
    SessionServices services;
    services.process = [&](const Desktop &) { ProcessState result; result.state = launches ? "running" : "stopped"; return result; };
    services.launch = [&](const Config &, const std::filesystem::path &, LaunchOptions, const Cancellation &, LaunchFacts *facts) {
        ++launches; facts->activationState = "returned"; facts->backendProxyConfig = "prepared";
        return Json{{"pid", 42}, {"instance", "reused"}, {"proxy_delivery", "unconfirmed"},
            {"activation_state", "returned"}, {"backend_proxy_config", "prepared"}, {"daemon_preparation", "skipped"}};
    };
    EngineSession session(path, [&](const Config &, const Cancellation &) { ++discoveries; Desktop result; result.displayName = "Fixture"; return result; }, normalUser, {}, services);
    TuiState state; TuiController controller(session, state); controller.refresh(); pump(controller);
    controller.handleKey(virtualKey(VK_RETURN)); controller.handleKey(key(L'e'));
    pump(controller); CHECK(launches == 1); CHECK(discoveries == 2);
    for (int count = 0; count < 10; ++count) controller.poll();
    CHECK(discoveries == 2); CHECK(state.modal() == Modal::LaunchResult);
    CHECK(state.diagnosticText().find("reused") != std::string::npos);
    CHECK(state.diagnosticText().find("unconfirmed") != std::string::npos);
    controller.handleKey(key(L'r')); pump(controller); CHECK(discoveries == 3);
    CHECK(state.diagnosticText().find("PID: 42") != std::string::npos);
}
TEST_CASE("RC3 modal borders survive long scope and confirmation remains inside")
{
    TuiState state; auto view = snapshotFixture();
    view.consent->home = L"C:/" + std::wstring(250, L'中') + L"/unique-tail";
    state.setSnapshot(view); state.key(key(L'b'));
    for (const wchar_t ch : std::wstring_view(L"YES")) state.key(key(ch));
    Screen screen; screen.resize(72, 22); state.draw(screen);
    // 68x17 centered => x=2, y=2; bottom border y=18.
    CHECK(screen.cell(2, 18).glyph == L"╰");
    CHECK(screen.cell(69, 18).glyph == L"╯");
    for (int x = 3; x < 69; ++x) CHECK(screen.cell(x, 18).glyph == L"─");
    for (int count = 0; count < 30; ++count) state.key(virtualKey(VK_DOWN));
    state.draw(screen);
    const auto frame = renderFrame(screen);
    // A full path may wrap in the middle of its final component. Read the
    // modal's visible interior cells rather than searching the VT/border stream.
    std::wstring visibleScope;
    for (int y = 3; y < 14; ++y)
        for (int x = 4; x < 68; ++x)
            if (!screen.cell(x, y).continuation && screen.cell(x, y).glyph != L" ")
                visibleScope += screen.cell(x, y).glyph;
    CHECK(visibleScope.find(L"unique-tail") != std::wstring::npos);
    CHECK(frame.find(L"YES_") != std::wstring::npos);
    state.clearModal(); state.key(key(L'e')); state.setEditorSeed(std::wstring(255, L'a'), L"7890");
    state.draw(screen); CHECK(renderFrame(screen).find(L"^") != std::wstring::npos);
}
TEST_CASE("RC3 terminal acquisition restores only acquired changed resources")
{
    for (int failure = 1; failure <= 2; ++failure) {
        FakeTerminal api; api.failGetMode = failure;
        { TerminalSession terminal(api); CHECK_FALSE(terminal.enable()); }
        CHECK(api.setModeCalls == 0); CHECK(api.cursorWrites == 0); CHECK(api.writes == 0);
    }
    FakeTerminal api; const auto originalInput = api.inputMode;
    { TerminalSession terminal(api); REQUIRE(terminal.enable());
      CHECK((api.inputMode & ENABLE_PROCESSED_INPUT) != 0);
      CHECK((api.inputMode & (ENABLE_QUICK_EDIT_MODE | ENABLE_VIRTUAL_TERMINAL_INPUT | ENABLE_ECHO_INPUT)) == 0);
      REQUIRE(terminal.enterAlternateScreen()); }
    CHECK(api.inputMode == originalInput); CHECK(api.outputMode == 0);
    CHECK(api.cursor.dwSize == 23); CHECK_FALSE(api.cursor.bVisible); CHECK(api.writes == 2);
    FakeTerminal failed; const auto failedInput = failed.inputMode; failed.failAlt = true;
    { TerminalSession terminal(failed); REQUIRE(terminal.enable()); CHECK_FALSE(terminal.enterAlternateScreen());
      CHECK(failed.inputMode == failedInput); CHECK(failed.outputMode == 0); }
    CHECK(failed.writes == 2);
    FakeTerminal partial; const auto originalPartial = partial.inputMode; partial.failSetMode = 2;
    { TerminalSession terminal(partial); CHECK_FALSE(terminal.enable()); }
    CHECK(partial.inputMode == originalPartial); CHECK(partial.outputMode == 0); CHECK(partial.cursorWrites == 0);
}
TEST_CASE("RC3 plain EOF is clean and R refresh D repair use the engine gate")
{
    cpgtest::TemporaryDirectory root;
    Config config; config.home = root.path;
    const auto path = root.file(L"guard.toml"); initializeConfig(path, config, false);
    int discoveries = 0;
    EngineSession session(path, [&](const Config &, const Cancellation &) -> Desktop { ++discoveries; throw Error("APPX_NOT_FOUND", "Fixture"); }, normalUser);
    std::istringstream input("r\nd\n"); std::ostringstream output;
    CHECK(runPlainConsoleSession(session, input, output) == 0);
    CHECK(discoveries == 2);
    CHECK(output.str().find("[D] Repair launch") != std::string::npos);
    CHECK(output.str().find("\x1b") == std::string::npos);
    CHECK_FALSE(Config::load(path).manageBackend);
}


TEST_CASE("RC3 diagnostics include bounded redacted error and retained receipt facts")
{
    TuiState state; state.setSnapshot(snapshotFixture());
    LaunchOutcome outcome; outcome.error = Error("ACTIVATION_FAILED", "Failed after daemon stopped");
    outcome.activationState = "outcome_unknown"; outcome.daemonPreparation = "stopped";
    outcome.backendProxyConfig = "prepared"; outcome.exceededBudget = true;
    state.setLaunchOutcome(outcome);
    const auto diagnostic = state.diagnosticText();
    CHECK(diagnostic.find("ACTIVATION_FAILED") != std::string::npos);
    CHECK(diagnostic.find("Failed after daemon stopped") != std::string::npos);
    CHECK(diagnostic.find("Daemon: stopped") != std::string::npos);
    CHECK(diagnostic.find("outcome_unknown") != std::string::npos);
    CHECK(diagnostic.find("\n") != std::string::npos);
    auto newer = snapshotFixture(); newer.error = Error("REFRESH_FAILED", "Discovery failed");
    state.setSnapshot(newer);
    CHECK(state.diagnosticText().find("ACTIVATION_FAILED") != std::string::npos);
    state.clearModal(); state.key(key(L'e')); state.setEditorSeed(L"localhost", L"7890"); state.beginSave(30);
    MutationOutcome late; late.operationId = 29; late.mutationError = Error("OLD", "Old request");
    state.applyMutationOutcome(late); CHECK(state.saving()); CHECK(state.editorError().empty());
}
TEST_CASE("RC3 plain failed output stops and invalid port does not mutate configuration")
{
    cpgtest::TemporaryDirectory root; Config config; config.home = root.path;
    const auto path = root.file(L"guard.toml"); initializeConfig(path, config, false);
    EngineSession session(path, absentDesktop, normalUser);
    std::istringstream empty; std::ostringstream failed; failed.setstate(std::ios::badbit);
    CHECK_THROWS_AS(runPlainConsoleSession(session, empty, failed), Error);
    std::istringstream input("e\nlocalhost\n7890abc\ne\n\n7890\nq\n"); std::ostringstream output;
    CHECK(runPlainConsoleSession(session, input, output) == 0);
    CHECK(Config::load(path).port == 7890);
    CHECK(output.str().find("Port must be a number") != std::string::npos);
    CHECK(output.str().find("host [localhost]") != std::string::npos);
    CHECK(output.str().find("Saved. Status refresh failed") != std::string::npos);
}


TEST_CASE("RC3 engine host error focuses Host after submission from Port")
{
    cpgtest::TemporaryDirectory root; Config config; config.home = root.path;
    const auto path = root.file(L"guard.toml"); initializeConfig(path, config, false);
    EngineSession session(path, absentDesktop, normalUser);
    TuiState state; TuiController controller(session, state); controller.refresh(); pump(controller);
    controller.handleKey(key(L'e')); state.setEditorSeed(L"remote.example", L"7890");
    controller.handleKey(virtualKey(VK_TAB)); REQUIRE(state.editorField() == 1);
    controller.handleKey(virtualKey(VK_RETURN)); pump(controller);
    CHECK(state.modal() == Modal::ProxyEditor); CHECK(state.editorField() == 0);
    CHECK(state.editorHost() == L"remote.example"); CHECK(Config::load(path).port == config.port);
}


TEST_CASE("RC3 production copy keeps launch receipt and pairs a failed automatic refresh error")
{
    cpgtest::TemporaryDirectory root; Config config; config.home = root.path;
    const auto path = root.file(L"guard.toml"); initializeConfig(path, config, false);
    std::atomic_int discoveries{0};
    SessionServices services;
    services.process = [](const Desktop &) { ProcessState result; result.state = "stopped"; return result; };
    services.launch = [](const Config &, const std::filesystem::path &, LaunchOptions, const Cancellation &, LaunchFacts *facts) {
        facts->activationState = "returned"; facts->backendProxyConfig = "prepared";
        return Json{{"pid", 51}, {"instance", "created"}, {"proxy_delivery", "activation_arguments"},
            {"activation_state", "returned"}, {"backend_proxy_config", "prepared"}, {"daemon_preparation", "skipped"}};
    };
    EngineSession session(path, [&](const Config &, const Cancellation &) {
        if (++discoveries > 1) throw Error("DISCOVERY_REFRESH_FAILED", "Read-only discovery fixture unavailable");
        Desktop result; result.displayName = "Fixture"; return result;
    }, normalUser, {}, services);
    std::wstring copied;
    bool copySucceeds = true;
    TuiState state; TuiController controller(session, state, [&](const std::wstring &text) { copied = text; return copySucceeds; });
    controller.refresh(); pump(controller); controller.handleKey(virtualKey(VK_RETURN)); pump(controller);
    REQUIRE(state.modal() == Modal::LaunchResult); CHECK(discoveries == 2);
    controller.handleKey(key(L'c'));
    CHECK(copied.find(L"PID: 51") != std::wstring::npos);
    CHECK(copied.find(L"Desktop activation returned a new instance") != std::wstring::npos);
    CHECK(copied.find(L"status.error.code: DISCOVERY_REFRESH_FAILED") != std::wstring::npos);
    CHECK(copied.find(L"status.error.message: Read-only discovery fixture unavailable") != std::wstring::npos);
    CHECK(copied.find(L"operation.error.code: DISCOVERY_REFRESH_FAILED") == std::wstring::npos);
    copySucceeds = false;
    controller.handleKey(key(L'c'));
    CHECK(state.modal() == Modal::LaunchResult);
    CHECK(state.notice().find("CLIPBOARD_FAILED") != std::string::npos);
    CHECK(state.diagnosticText().find("PID: 51") != std::string::npos);
    CHECK(state.diagnosticText().find("Daemon: skipped") != std::string::npos);
}


TEST_CASE("RC3 synchronous consent read failure clears controller proposal before another B")
{
    cpgtest::TemporaryDirectory root; Config config; config.home = root.path;
    const auto path = root.file(L"guard.toml"); initializeConfig(path, config, false);
    EngineSession session(path, absentDesktop, normalUser);
    TuiState state; TuiController controller(session, state); controller.refresh(); pump(controller);
    controller.handleKey(key(L'b')); REQUIRE(state.consentProposal());
    const HANDLE lock = CreateFileW(path.c_str(), GENERIC_READ, 0, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    REQUIRE(lock != INVALID_HANDLE_VALUE);
    yes(controller); CloseHandle(lock);
    CHECK_FALSE(state.snapshot().consent.has_value());
    controller.handleKey(key(L'b')); CHECK(state.modal() == Modal::ErrorResult);
    CHECK_FALSE(Config::load(path).manageBackend);
}


namespace {
struct FakePlainInput : PlainConsoleInput {
    DWORD mode = ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_QUICK_EDIT_MODE | ENABLE_VIRTUAL_TERMINAL_INPUT;
    int modeWrites = 0, nextCalls = 0, newlines = 0;
    bool failGet = false, failSet = false, failNext = false, failEcho = false;
    std::vector<PlainInputEvent> events;
    std::wstring echoed;
    bool getMode(DWORD &value) override { value = mode; return !failGet; }
    bool setMode(DWORD value) override { ++modeWrites; if (failSet) return false; mode = value; return true; }
    PlainInputEvent next() override {
        if (failNext) throw Error("CONSOLE_INPUT_FAILED", "Fixture wait failed");
        if (static_cast<std::size_t>(nextCalls) >= events.size()) throw Error("TEST_FAILED", "Input consumed after close/Enter");
        return events[static_cast<std::size_t>(nextCalls++)];
    }
    void echo(std::wstring_view, std::wstring_view value) override {
        if (failEcho) throw Error("CONSOLE_OUTPUT_FAILED", "Fixture echo failed");
        echoed = value;
    }
    void newline() override { ++newlines; }
};
PlainInputEvent inputKey(Key value) { return {PlainInputEvent::Kind::Key, value}; }
}
TEST_CASE("RC3 plain key reader handles Unicode backspace cancel and EOF and restores input mode")
{
    FakePlainInput input; const auto original = input.mode;
    input.events = {inputKey(key(L'e')), {PlainInputEvent::Kind::Cancel, {}},
        inputKey(key(L'中')), inputKey(key(0xd83d)), inputKey(key(0xde42)),
        inputKey(virtualKey(VK_BACK)), inputKey(key(L'x')), inputKey(virtualKey(VK_RETURN)), inputKey(key(L'z'))};
    const auto line = readPlainConsoleLine(input);
    REQUIRE(line); CHECK(*line == L"中x"); CHECK(input.echoed == L"中x");
    CHECK(input.nextCalls == 8); CHECK(input.mode == original); CHECK(input.modeWrites == 2);
    FakePlainInput closed; const auto closedMode = closed.mode;
    closed.events = {{PlainInputEvent::Kind::Close, {}}, inputKey(key(L'e'))};
    CHECK_FALSE(readPlainConsoleLine(closed)); CHECK(closed.nextCalls == 1); CHECK(closed.mode == closedMode);
    FakePlainInput eof; Key end; end.control = true; end.virtualKey = 'Z'; end.unicode = 0x1a;
    eof.events = {inputKey(end)}; CHECK_FALSE(readPlainConsoleLine(eof)); CHECK(eof.newlines == 1);
}
TEST_CASE("RC3 plain line reader mode acquisition and read or echo failures are bounded")
{
    FakePlainInput getFailed; getFailed.failGet = true;
    CHECK_THROWS_AS(readPlainConsoleLine(getFailed), Error); CHECK(getFailed.modeWrites == 0);
    FakePlainInput setFailed; const auto original = setFailed.mode; setFailed.failSet = true;
    CHECK_THROWS_AS(readPlainConsoleLine(setFailed), Error); CHECK(setFailed.modeWrites == 1); CHECK(setFailed.mode == original);
    FakePlainInput readFailed; readFailed.failNext = true;
    CHECK_THROWS_AS(readPlainConsoleLine(readFailed), Error); CHECK(readFailed.modeWrites == 2); CHECK(readFailed.mode == original);
    FakePlainInput echoFailed; echoFailed.failEcho = true; echoFailed.events = {inputKey(key(L'x'))};
    CHECK_THROWS_AS(readPlainConsoleLine(echoFailed), Error); CHECK(echoFailed.modeWrites == 2); CHECK(echoFailed.mode == original);
    PlainLineEditor editor;
    for (int count = 0; count < 4096; ++count) editor.handle(key(L'x'));
    CHECK_THROWS_AS(editor.handle(key(L'x')), Error);
    CHECK(editor.value().size() == 4096);
    editor.clear(); editor.handle(key(0xd83d)); editor.handle(virtualKey(VK_RETURN));
    CHECK(editor.value() == L"\xfffd");
}
TEST_CASE("RC3 plain exceptional exit reaps an externally owned session before control release")
{
    cpgtest::TemporaryDirectory root; Config config; config.home = root.path;
    const auto path = root.file(L"guard.toml"); initializeConfig(path, config, false);
    std::atomic_bool entered{false}, settled{false};
    EngineSession session(path, [&](const Config &, const Cancellation &cancel) -> Desktop {
        entered = true;
        while (!cancel.cancelled()) Sleep(1);
        Sleep(15); settled = true; cancel.check(); return {};
    }, normalUser);
    session.requestRefresh();
    for (int count = 0; count < 2000 && !entered; ++count) Sleep(1);
    REQUIRE(entered);
    std::istringstream input; std::ostringstream failed; failed.setstate(std::ios::badbit);
    CHECK_THROWS_AS(runPlainConsoleSession(session, input, failed), Error);
    CHECK(settled); CHECK_FALSE(session.busy());
}
TEST_CASE("RC3 native plain input wakes on real Ctrl C without a pending Enter")
{
    wchar_t executable[32768]{}; REQUIRE(GetModuleFileNameW(nullptr, executable, 32768) > 0);
    std::wstring command = L"\"" + std::wstring(executable) + L"\" --plain-control-test";
    STARTUPINFOW startup{}; startup.cb = sizeof startup;
    startup.dwFlags = STARTF_USESHOWWINDOW; startup.wShowWindow = SW_HIDE;
    PROCESS_INFORMATION child{};
    REQUIRE(CreateProcessW(executable, command.data(), nullptr, nullptr, FALSE,
        CREATE_NEW_CONSOLE, nullptr, nullptr, &startup, &child));
    UniqueHandle process(child.hProcess), thread(child.hThread);
    const DWORD wait = WaitForSingleObject(process.get(), 10000);
    if (wait != WAIT_OBJECT_0) {
        CHECK(TerminateProcess(process.get(), 99));
        CHECK(WaitForSingleObject(process.get(), 5000) == WAIT_OBJECT_0);
    }
    REQUIRE(wait == WAIT_OBJECT_0); DWORD exitCode = 99;
    REQUIRE(GetExitCodeProcess(process.get(), &exitCode)); CHECK(exitCode == 0);
}

TEST_CASE("RC3 native plain close wakes idle input and restores mode before close acknowledgement")
{
    SECURITY_ATTRIBUTES attributes{sizeof(SECURITY_ATTRIBUTES), nullptr, TRUE};
    UniqueHandle completed(CreateEventW(&attributes, TRUE, FALSE, nullptr)); REQUIRE(completed);
    wchar_t executable[32768]{}; REQUIRE(GetModuleFileNameW(nullptr, executable, 32768) > 0);
    std::wstring command = L"\"" + std::wstring(executable) + L"\" --plain-close-test "
        + std::to_wstring(reinterpret_cast<std::uintptr_t>(completed.get()));
    STARTUPINFOW startup{}; startup.cb = sizeof startup;
    startup.dwFlags = STARTF_USESHOWWINDOW; startup.wShowWindow = SW_HIDE;
    PROCESS_INFORMATION child{};
    REQUIRE(CreateProcessW(executable, command.data(), nullptr, nullptr, TRUE,
        CREATE_NEW_CONSOLE, nullptr, nullptr, &startup, &child));
    UniqueHandle process(child.hProcess), thread(child.hThread);
    const DWORD wait = WaitForSingleObject(process.get(), 10000);
    if (wait != WAIT_OBJECT_0) {
        CHECK(TerminateProcess(process.get(), 99));
        CHECK(WaitForSingleObject(process.get(), 5000) == WAIT_OBJECT_0);
    }
    REQUIRE(wait == WAIT_OBJECT_0);
    CHECK(WaitForSingleObject(completed.get(), 0) == WAIT_OBJECT_0);
}

TEST_CASE("RC3 production VT scope restores terminal before acknowledging real close")
{
    SECURITY_ATTRIBUTES attributes{sizeof(SECURITY_ATTRIBUTES), nullptr, TRUE};
    UniqueHandle completed(CreateEventW(&attributes, TRUE, FALSE, nullptr)); REQUIRE(completed);
    wchar_t executable[32768]{}; REQUIRE(GetModuleFileNameW(nullptr, executable, 32768) > 0);
    std::wstring command = L"\"" + std::wstring(executable) + L"\" --vt-scope-close-test "
        + std::to_wstring(reinterpret_cast<std::uintptr_t>(completed.get()));
    STARTUPINFOW startup{}; startup.cb = sizeof startup;
    startup.dwFlags = STARTF_USESHOWWINDOW; startup.wShowWindow = SW_HIDE;
    PROCESS_INFORMATION child{};
    REQUIRE(CreateProcessW(executable, command.data(), nullptr, nullptr, TRUE,
        CREATE_NEW_CONSOLE, nullptr, nullptr, &startup, &child));
    UniqueHandle process(child.hProcess), thread(child.hThread);
    const DWORD wait = WaitForSingleObject(process.get(), 10000);
    if (wait != WAIT_OBJECT_0) {
        CHECK(TerminateProcess(process.get(), 99)); CHECK(WaitForSingleObject(process.get(), 5000) == WAIT_OBJECT_0);
    }
    REQUIRE(wait == WAIT_OBJECT_0);
    CHECK(WaitForSingleObject(completed.get(), 0) == WAIT_OBJECT_0);
}
TEST_CASE("RC3 redirected live pipe cancels partial lines and closes without waiting for EOF")
{
    wchar_t executable[32768]{}; REQUIRE(GetModuleFileNameW(nullptr, executable, 32768) > 0);
    for (const auto *mode : {L"cancel", L"close"}) {
        SECURITY_ATTRIBUTES attributes{sizeof(SECURITY_ATTRIBUTES), nullptr, TRUE};
        UniqueHandle completed(CreateEventW(&attributes, TRUE, FALSE, nullptr)); REQUIRE(completed);
        std::wstring command = L"\"" + std::wstring(executable) + L"\" --plain-pipe-control-test " + mode + L" "
            + std::to_wstring(reinterpret_cast<std::uintptr_t>(completed.get()));
        STARTUPINFOW startup{}; startup.cb = sizeof startup;
        startup.dwFlags = STARTF_USESHOWWINDOW; startup.wShowWindow = SW_HIDE;
        PROCESS_INFORMATION child{};
        REQUIRE(CreateProcessW(executable, command.data(), nullptr, nullptr, TRUE,
            CREATE_NEW_CONSOLE, nullptr, nullptr, &startup, &child));
        UniqueHandle process(child.hProcess), thread(child.hThread);
        const DWORD wait = WaitForSingleObject(process.get(), 10000);
        if (wait != WAIT_OBJECT_0) {
            CHECK(TerminateProcess(process.get(), 99)); CHECK(WaitForSingleObject(process.get(), 5000) == WAIT_OBJECT_0);
        }
        REQUIRE(wait == WAIT_OBJECT_0);
        CHECK(WaitForSingleObject(completed.get(), 0) == WAIT_OBJECT_0);
    }
}
TEST_CASE("RC3 redirected input rejects invalid and non-console character handles")
{
    ConsoleControlScope controls;
    const auto rejected = [&](HANDLE handle) {
        try {
            (void)cpg::tui::detail::readRedirectedPlainLine(handle, controls);
        } catch (const Error &error) {
            CHECK(error.code == "CONSOLE_INPUT_FAILED");
            return;
        }
        FAIL("unsupported redirected input was accepted");
    };
    rejected(INVALID_HANDLE_VALUE);
    rejected(nullptr);
    UniqueHandle nul(CreateFileW(L"NUL", GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE,
        nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
    REQUIRE(nul);
    REQUIRE(GetFileType(nul.get()) == FILE_TYPE_CHAR);
    DWORD mode = 0;
    REQUIRE_FALSE(GetConsoleMode(nul.get(), &mode));
    rejected(nul.get());
}
namespace {
struct PlainReadHandshake final : cpg::tui::detail::PlainReadObserver {
    UniqueHandle ready{CreateEventW(nullptr, TRUE, FALSE, nullptr)};
    UniqueHandle partial{CreateEventW(nullptr, TRUE, FALSE, nullptr)};
    UniqueHandle cancelled{CreateEventW(nullptr, TRUE, FALSE, nullptr)};
    std::atomic_int phase{0};
    std::atomic_bool valid{true};
    PlainReadHandshake() {
        if (!ready || !partial || !cancelled) throw Error("TEST_FAILED", "Cannot create reader handshake events.");
    }
    void progressed(cpg::tui::detail::PlainReadStage stage, std::size_t units) override {
        using Stage = cpg::tui::detail::PlainReadStage;
        if (stage == Stage::Ready) {
            if (phase != 0 || units != 0) valid = false;
            phase = 1; SetEvent(ready.get());
        } else if (stage == Stage::Cancelled) {
            if (phase != 2 || units != 0) valid = false;
            phase = 3; SetEvent(cancelled.get());
        } else if (phase == 1 && units == 1) {
            phase = 2; SetEvent(partial.get());
        } else if (phase == 3 && units == 1) {
            phase = 4; // the post-cancel q was actually consumed
        } else valid = false;
    }
    bool completed() const { return valid && phase == 4; }
};
template<class Write>
void drivePlainCancellation(PlainReadHandshake &read, Write write)
{
    // All transitions are acknowledged by the production reader, not elapsed
    // time or asynchronous GenerateConsoleCtrlEvent submission alone.
    const auto reached = [&](HANDLE event) {
        return WaitForSingleObject(event, 2000) == WAIT_OBJECT_0 && read.valid;
    };
    if (reached(read.ready.get()) && write("e") && reached(read.partial.get())
        && GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) && reached(read.cancelled.get())
        && write("q\n")) return;
    read.valid = false;
    // Unblock the owned child's reader on a failed handshake. The parent still
    // rejects its missing success receipt/nonzero exit; no retry is performed.
    PostMessageW(GetConsoleWindow(), WM_CLOSE, 0, 0);
}
}
int main(int argc, char **argv)
{
    if (argc == 3 && std::string(argv[1]) == "--vt-scope-close-test") {
        try {
            struct RestoreReceiptTerminal : FakeTerminal {
                HANDLE completion = nullptr;
                DWORD originalInput = inputMode;
                bool setMode(HANDLE handle, DWORD mode) override {
                    const bool ok = FakeTerminal::setMode(handle, mode);
                    if (ok && setModeCalls == 4 && inputMode == originalInput && outputMode == 0
                        && cursorWrites == 2 && writes == 2 && cursor.bVisible == FALSE)
                        SetEvent(completion);
                    return ok;
                }
            } terminal;
            terminal.completion = reinterpret_cast<HANDLE>(static_cast<std::uintptr_t>(std::stoull(argv[2])));
            cpg::tui::detail::InteractiveTerminalScope scope(terminal);
            if (!scope.terminal.enable() || !scope.terminal.enterAlternateScreen()) return 31;
            std::thread closer([] { Sleep(200); PostMessageW(GetConsoleWindow(), WM_CLOSE, 0, 0); });
            const DWORD wait = WaitForSingleObject(scope.controls.signalEvent(), 5000);
            closer.join();
            if (wait != WAIT_OBJECT_0 || !scope.controls.takeClose()) return 32;
            return 0; // the production scope destructor writes the restoration receipt
        } catch (...) { return 33; }
    }
    if (argc == 4 && std::string(argv[1]) == "--plain-pipe-control-test") {
        try {
            const bool closing = std::string(argv[2]) == "close";
            const HANDLE completed = reinterpret_cast<HANDLE>(static_cast<std::uintptr_t>(std::stoull(argv[3])));
            HANDLE read = nullptr, write = nullptr;
            if (!CreatePipe(&read, &write, nullptr, 4096)) return 41;
            UniqueHandle pipeRead(read), pipeWrite(write); // write remains open throughout the read
            if (!SetStdHandle(STD_INPUT_HANDLE, pipeRead.get())) return 42;
            if (closing) {
                ConsoleControlScope controls;
                std::thread closer([] { Sleep(200); PostMessageW(GetConsoleWindow(), WM_CLOSE, 0, 0); });
                std::optional<std::string> value;
                try { value = cpg::tui::detail::readRedirectedPlainLine(pipeRead.get(), controls); }
                catch (...) { closer.join(); return 43; }
                closer.join();
                if (value || !SetEvent(completed)) return 44;
                return 0;
            }
            cpgtest::TemporaryDirectory root; Config config; config.home = root.path;
            const auto path = root.file(L"guard.toml"); initializeConfig(path, config, false);
            EngineSession session(path, absentDesktop, normalUser);
            PlainReadHandshake handshake;
            std::thread writer([&] {
                drivePlainCancellation(handshake, [&](std::string_view text) {
                    DWORD written = 0;
                    return WriteFile(pipeWrite.get(), text.data(), static_cast<DWORD>(text.size()), &written, nullptr)
                        && written == text.size();
                });
            });
            int result = 45;
            try { result = runPlainConsoleSession(session, std::cin, std::cout, &handshake); }
            catch (...) { writer.join(); return 46; }
            writer.join();
            if (result != 0 || !handshake.completed() || session.busy() || Config::load(path).manageBackend) return 47;
            return SetEvent(completed) ? 0 : 48;
        } catch (...) { return 49; }
    }

    if (argc == 3 && std::string(argv[1]) == "--plain-close-test") {
        try {
            const HANDLE completed = reinterpret_cast<HANDLE>(static_cast<std::uintptr_t>(std::stoull(argv[2])));
            ConsoleControlScope controls;
            DWORD originalMode = 0; if (!GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &originalMode)) return 21;
            std::thread closer([] { Sleep(200); PostMessageW(GetConsoleWindow(), WM_CLOSE, 0, 0); });
            std::optional<std::wstring> value;
            try { value = readNativePlainConsoleLine(controls); }
            catch (...) { closer.join(); return 22; }
            closer.join();
            DWORD restored = 0;
            if (value || !GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &restored) || restored != originalMode) return 23;
            if (!SetEvent(completed)) return 24;
            return 0; // acknowledgement occurs after the above restoration proof
        } catch (...) { return 25; }
    }

    if (argc == 2 && std::string(argv[1]) == "--plain-control-test") {
        try {
            cpgtest::TemporaryDirectory root; Config config; config.home = root.path;
            const auto path = root.file(L"guard.toml"); initializeConfig(path, config, false);
            EngineSession session(path, absentDesktop, normalUser);
            DWORD originalMode = 0; if (!GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &originalMode)) return 11;
            PlainReadHandshake read;
            std::thread writer([&] {
                drivePlainCancellation(read, [&](std::string_view text) {
                    for (const char ch : text) {
                        INPUT_RECORD record{}; record.EventType = KEY_EVENT;
                        record.Event.KeyEvent.bKeyDown = TRUE; record.Event.KeyEvent.wRepeatCount = 1;
                        record.Event.KeyEvent.uChar.UnicodeChar = ch == '\n' ? L'\r' : static_cast<wchar_t>(ch);
                        record.Event.KeyEvent.wVirtualKeyCode = ch == '\n' ? static_cast<WORD>(VK_RETURN)
                            : static_cast<WORD>(std::towupper(ch));
                        DWORD count = 0;
                        if (!WriteConsoleInputW(GetStdHandle(STD_INPUT_HANDLE), &record, 1, &count) || count != 1) return false;
                    }
                    return true;
                });
            });
            int result = 12;
            try { result = runPlainConsoleSession(session, std::cin, std::cout, &read); }
            catch (...) { writer.join(); return 13; }
            writer.join();
            DWORD restored = 0;
            if (!GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &restored) || restored != originalMode) return 14;
            return result == 0 && read.completed() && !session.busy() && !Config::load(path).manageBackend ? 0 : 15;
        } catch (...) { return 16; }
    }
    return cpgtest::runDoctestMain(argc, argv);
}
