#include "tui_state.h"
#include "display_width.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

#include <algorithm>
#include <cwctype>

#include "cpg/text.h"

namespace cpg::tui {
namespace {

// Missing keys resolve to a shared empty OBJECT (not null): nlohmann's
// value() throws on a null receiver, and the TUI must render before the
// first session snapshot arrives.
const Json &at(const Json &object, const char *key)
{
    static const Json empty = Json::object();
    const auto entry = object.find(key);
    return entry == object.end() ? empty : *entry;
}

std::wstring widen(const std::string &utf8)
{
    if (utf8.empty()) return {};
    const int size = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, utf8.c_str(),
                                         static_cast<int>(utf8.size()), nullptr, 0);
    if (size <= 0) return {};
    std::wstring wide(static_cast<std::size_t>(size), L'\0');
    MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, utf8.c_str(),
                        static_cast<int>(utf8.size()), wide.data(), size);
    return wide;
}

std::uint8_t stateStyle(const std::string &state)
{
    if (state == "ready" || state == "current" || state == "running" || state == "stopped"
        || state == "found" || state == "prepared")
        return StyleGreen;
    if (state == "pending" || state == "stale" || state == "not_authorized"
        || state == "repair_required" || state == "unknown")
        return state == "unknown" ? StyleDim : StyleYellow;
    return StyleRed; // conflict/invalid/blocked/not_found/unsupported
}

// Draws a bounded centered box with a title; returns its inner origin.
struct Box {
    int x = 0, y = 0, width = 0, height = 0;
};

Box drawBox(Screen &screen, const std::wstring &title, int width, int height)
{
    width = std::min(width, screen.width() - 2);
    height = std::min(height, screen.height() - 2);
    const int x = std::max(0, (screen.width() - width) / 2);
    const int y = std::max(0, (screen.height() - height) / 2);
    for (int row = 0; row < height && y + row < screen.height(); ++row) {
        std::wstring line(static_cast<std::size_t>(std::min(width, screen.width() - x)), L' ');
        const bool edge = row == 0 || row == height - 1;
        if (edge) {
            for (int column = 0; column < static_cast<int>(line.size()); ++column)
                line[static_cast<std::size_t>(column)] = L'\x2500';
        }
        line.front() = edge ? L'\x256d' : L'\x2502';
        line.back() = edge ? L'\x256e' : L'\x2502';
        if (row == height - 1 && height > 1) {
            line.front() = L'\x2570';
            line.back() = L'\x256f';
        }
        screen.text(x, y + row, line, StyleDim);
    }
    if (!title.empty() && height > 0)
        screen.text(x + 2, y, L" " + title + L" ", StyleBold);
    return Box{x + 2, y + 1, width - 4, height - 2};
}

} // namespace

void TuiState::setBusy(bool busy, const std::string &label)
{
    if (busy && !busy_) modal_ = Modal::None; // entering busy closes modals
    if (busy) notice_.clear();
    busy_ = busy;
    busyLabel_ = label;
}

void TuiState::setOperationOutcome(const Error *error)
{
    notice_.clear();
    hasOutcome_ = true;
    outcomeOk_ = error == nullptr;
    outcomeCode_.clear();
    outcomeMessage_.clear();
    if (error) {
        outcomeCode_ = error->code;
        outcomeMessage_ = error->message;
        modal_ = Modal::ErrorResult;
    } else {
        modal_ = Modal::LaunchResult;
    }
}

void TuiState::setEditorSeed(const std::wstring &host, const std::wstring &port)
{
    editor_.field = 0;
    editor_.host = host;
    editor_.port = port;
    editor_.hostCursor = host.size();
    editor_.portCursor = port.size();
    editorError_.clear();
}

TuiState::Action TuiState::key(const Key &key)
{
    switch (modal_) {
    case Modal::None: return mainKey(key);
    case Modal::ProxyEditor: return editorKey(key);
    case Modal::Consent:
    case Modal::Repair: return confirmKey(key);
    case Modal::ErrorResult: {
        Action action;
        if (key.unicode == L'E' || key.unicode == L'e') {
            modal_ = Modal::ProxyEditor;
            action.kind = Action::Kind::OpenEditor;
            return action;
        }
        if (key.unicode == L'B' || key.unicode == L'b') {
            modal_ = Modal::Consent;
            confirmBuffer_.clear();
            action.kind = Action::Kind::OpenConsent;
            return action;
        }
        if (key.unicode == L'C' || key.unicode == L'c') { action.kind = Action::Kind::CopyDiagnostic; return action; }
        if (key.unicode == L'R' || key.unicode == L'r') { action.kind = Action::Kind::Refresh; return action; }
        if (key.virtualKey == VK_ESCAPE || key.virtualKey == VK_RETURN) modal_ = Modal::None;
        return {};
    }
    case Modal::LaunchResult:
    case Modal::Help:
        modal_ = Modal::None;
        return {};
    case Modal::QuitWhileBusy:
        if (key.unicode == L'Y' || key.unicode == L'y'
            || key.virtualKey == VK_RETURN) {
            Action action;
            action.kind = Action::Kind::Cancel;
            modal_ = Modal::None;
            return action;
        }
        modal_ = Modal::None;
        return {};
    }
    return {};
}

TuiState::Action TuiState::mainKey(const Key &key)
{
    Action action;
    if (key.control && (key.unicode == 0x03 || key.virtualKey == VK_CANCEL)) {
        if (busy_) { action.kind = Action::Kind::Cancel; return action; }
        return {};
    }
    if (busy_) {
        // Only cancellation and the busy-quit prompt are accepted while a
        // foreground operation owns the engine.
        if (key.unicode == L'Q' || key.unicode == L'q') { modal_ = Modal::QuitWhileBusy; return {}; }
        return {};
    }
    switch (key.virtualKey) {
    case VK_RETURN:
        action.kind = Action::Kind::Launch;
        return action;
    case VK_F5:
        action.kind = Action::Kind::Refresh;
        return action;
    case VK_ESCAPE:
        return {};
    }
    switch (std::towlower(key.unicode)) {
    case L'r': action.kind = Action::Kind::Refresh; return action;
    case L'e': modal_ = Modal::ProxyEditor; action.kind = Action::Kind::OpenEditor; return action;
    case L'b': modal_ = Modal::Consent; confirmBuffer_.clear(); action.kind = Action::Kind::OpenConsent; return action;
    case L'd': modal_ = Modal::Repair; confirmBuffer_.clear(); action.kind = Action::Kind::OpenRepair; return action;
    case L'?':
    case 0: // F1 has no unicode
        if (key.virtualKey == VK_F1 || key.unicode == L'?') { modal_ = Modal::Help; }
        return {};
    case L'q': action.kind = Action::Kind::Quit; return action;
    }
    return {};
}

TuiState::Action TuiState::editorKey(const Key &key)
{
    Action action;
    if (key.virtualKey == VK_ESCAPE) {
        modal_ = Modal::None;
        return {};
    }
    if (key.virtualKey == VK_TAB) {
        editor_.field = 1 - editor_.field;
        return {};
    }
    std::wstring &value = editor_.field == 0 ? editor_.host : editor_.port;
    std::size_t &cursor = editor_.field == 0 ? editor_.hostCursor : editor_.portCursor;
    if (key.virtualKey == VK_RETURN) {
        try {
            // Port fields hold ASCII digits only (enforced on input), so a
            // plain widen-per-digit conversion avoids any narrowing.
            std::string digits;
            for (const wchar_t character : value) {
                if (character < L'0' || character > L'9')
                    throw std::invalid_argument("digits");
                digits.push_back(static_cast<char>(character));
            }
            const int port = std::stoi(digits);
            if (port < 1 || port > 65535) throw std::invalid_argument("range");
            action.kind = Action::Kind::SaveProxy;
            action.host = editor_.host;
            action.port = port;
            return action;
        } catch (const std::exception &) {
            editorError_ = L"Port must be a number between 1 and 65535.";
            return {};
        }
    }
    if (key.virtualKey == VK_LEFT && cursor > 0) { --cursor; return {}; }
    if (key.virtualKey == VK_RIGHT && cursor < value.size()) { ++cursor; return {}; }
    if (key.virtualKey == VK_HOME) { cursor = 0; return {}; }
    if (key.virtualKey == VK_END) { cursor = value.size(); return {}; }
    if (key.virtualKey == VK_BACK && cursor > 0) { value.erase(--cursor, 1); return {}; }
    if (key.virtualKey == VK_DELETE && cursor < value.size()) { value.erase(cursor, 1); return {}; }
    if (key.unicode >= 0x20) {
        const std::size_t limit = editor_.field == 0 ? 255 : 5;
        if (value.size() >= limit) return {};
        if (editor_.field == 1 && (key.unicode < L'0' || key.unicode > L'9')) return {};
        value.insert(cursor++, 1, key.unicode);
        return {};
    }
    return {};
}

TuiState::Action TuiState::confirmKey(const Key &key)
{
    Action action;
    if (key.virtualKey == VK_ESCAPE) {
        modal_ = Modal::None;
        confirmBuffer_.clear();
        return {};
    }
    if (key.virtualKey == VK_BACK && !confirmBuffer_.empty()) {
        confirmBuffer_.pop_back();
        return {};
    }
    if (key.virtualKey == VK_RETURN) {
        if (confirmBuffer_ == L"YES") {
            const bool repair = modal_ == Modal::Repair;
            modal_ = Modal::None;
            confirmBuffer_.clear();
            if (repair) {
                action.kind = Action::Kind::ConfirmRepair;
                action.token = at(at(snapshot_, "confirmations"), "repair").value("token", "");
            } else {
                action.kind = Action::Kind::ConfirmConsent;
                action.enableConsent = !at(snapshot_, "coverage").value("enabled", false);
                action.token = at(at(snapshot_, "confirmations"), "backend_proxy").value("token", "");
            }
            return action;
        }
        confirmBuffer_.clear();
        return {};
    }
    if (key.unicode >= 0x20 && key.unicode < 0x7f && confirmBuffer_.size() < 8)
        confirmBuffer_.push_back(static_cast<wchar_t>(std::towupper(key.unicode)));
    return {};
}

std::wstring TuiState::blockedReason(const Json &snapshot)
{
    const Json &error = at(snapshot, "error");
    if (error.is_null()) return {};
    const std::string code = error.value("code", "");
    const Json &process = at(snapshot, "process");
    const std::string processState = process.value("state", "");
    if (code == "CONFIG_INVALID")
        return L"Proxy configuration needs repair - press E to edit it.";
    if (code == "ELEVATION_NOT_ALLOWED")
        return L"Run Guard as a normal user; elevation is not allowed.";
    if (processState == "running")
        return L"Desktop is running - fully exit it before launching.";
    if (processState != "stopped")
        return L"Cannot confirm whether Desktop is running.";
    if (!code.empty()) return widen(code) + L" - press R to refresh.";
    return L"Desktop not found - press R to refresh.";
}

void TuiState::drawMain(Screen &screen) const
{
    const Json &proxy = at(snapshot_, "proxy");
    const Json &desktop = at(snapshot_, "desktop");
    const Json &process = at(snapshot_, "process");
    const Json &coverage = at(snapshot_, "coverage");
    const Json &actions = at(snapshot_, "actions");
    const bool canLaunch = actions.value("can_launch", false);

    screen.text(0, 0, L"CODEX PROXY GUARD", StyleBold);
    std::wstring badge;
    std::uint8_t badgeStyle = StyleDim;
    if (busy_) {
        static const wchar_t *spinner = L"|/-\\";
        badge = std::wstring(L"\x25cf ") + spinner[spinnerPhase_ % 4] + L" " + widen(busyLabel_);
        badgeStyle = StyleYellow;
    } else if (canLaunch) {
        badge = L"\x25cf READY";
        badgeStyle = StyleGreen;
    } else {
        badge = L"\x25cf BLOCKED";
        badgeStyle = StyleRed;
    }
    std::wstring padded;
    appendTruncated(padded, badge, screen.width() - 19);
    screen.text(std::max(19, screen.width() - static_cast<int>(displayWidth(padded))), 0, padded, badgeStyle);
    screen.text(0, 1, L"Local proxy launcher", StyleDim);
    screen.rule(2, StyleDim);

    const int rows = screen.height() - 6;
    auto row = [&](int index, const std::wstring &label, const std::wstring &value, std::uint8_t style) {
        if (index >= rows) return;
        screen.text(1, 3 + index, label, StyleDim);
        screen.text(13, 3 + index, value, style);
    };
    const std::string coverageState = coverage.value("state", "unknown");
    std::wstring coverageText;
    if (!desktop.value("state", "").empty() && desktop.value("state", "") == "not_applicable")
        coverageText = L"process environment";
    else if (coverageState == "not_applicable")
        coverageText = L"process environment";
    else {
        coverageText = widen(coverageState);
        if (coverage.value("enabled", false)) {
            const std::string home = coverage.value("authorized_home", "");
            coverageText += L" - " + widen(home);
        } else if (coverageState == "not_authorized") {
            coverageText += L" (press B to authorize)";
        }
    }
    row(0, L"Proxy", widen(proxy.value("url", "")), StyleDefault);
    row(1, L"Launch", widen(at(snapshot_, "launch").value("method", "unknown")), StyleDefault);
    row(2, L"App",
        widen(desktop.value("display_name", "")) + L" \x00b7 "
            + widen(desktop.value("package_version", "")) + L" \x00b7 "
            + widen(desktop.value("architecture", "")),
        desktop.value("state", "") == "found" ? StyleDefault : StyleDim);
    row(3, L"Entry",
        widen(desktop.value("application_id", "-")) + L" \x00b7 "
            + widen(desktop.value("runtime_kind", "-")),
        StyleDefault);
    row(4, L"Process", widen(process.value("state", "unknown")),
        stateStyle(process.value("state", "unknown")));
    row(5, L"Coverage", coverageText, stateStyle(coverageState));

    screen.rule(screen.height() - 3, StyleDim);
    if (busy_) {
        screen.text(1, screen.height() - 2, L"[ Ctrl+C ] Cancel " + widen(busyLabel_), StyleYellow);
    } else if (!notice_.empty()) {
        std::wstring notice;
        appendTruncated(notice, widen(notice_), screen.width() - 2);
        screen.text(1, screen.height() - 2, notice, StyleGreen);
    } else if (canLaunch) {
        screen.text(1, screen.height() - 2,
                    L"[ Enter ] Launch " + widen(desktop.value("display_name", "Desktop")), StyleBold);
    } else {
        std::wstring reason;
        appendTruncated(reason, blockedReason(snapshot_), screen.width() - 2);
        screen.text(1, screen.height() - 2, reason, StyleYellow);
    }
    screen.text(1, screen.height() - 1,
                L"E Proxy  B Codex Proxy  D Repair  R Refresh  ? Help  Q Quit", StyleDim);
}

void TuiState::drawModal(Screen &screen) const
{
    switch (modal_) {
    case Modal::None:
        return;
    case Modal::ProxyEditor: {
        const Box box = drawBox(screen, L"Proxy Configuration", 58, 8);
        std::wstring hostLine = L"Host  [" + editor_.host + L"]";
        std::wstring portLine = L"Port  [" + editor_.port + L"]";
        screen.text(box.x, box.y, hostLine, editor_.field == 0 ? StyleBold : StyleDefault);
        screen.text(box.x + 13 + static_cast<int>(editor_.hostCursor), box.y, L"", StyleDefault);
        screen.text(box.x, box.y + 1, portLine, editor_.field == 1 ? StyleBold : StyleDefault);
        std::wstring hint = editorError_.empty()
            ? L"Tab switch \x00b7 Enter save \x00b7 Esc cancel"
            : editorError_;
        screen.text(box.x, box.y + 3, hint, editorError_.empty() ? StyleDim : StyleRed);
        break;
    }
    case Modal::Consent: {
        const Box box = drawBox(screen, L"Backend Proxy Authorization", 62, 9);
        const std::wstring home = confirmHome_;
        const bool enabled = at(snapshot_, "coverage").value("enabled", false);
        const std::wstring lines[] = {
            enabled ? L"Remove Guard's unchanged proxy block from:" :
                      L"Authorize HTTP_PROXY/HTTPS_PROXY/NO_PROXY block for later",
            enabled ? home : L"Codex processes in:",
            enabled ? L"" : home,
            L"Only the marked Guard block is managed; it affects later",
            L"Codex processes sharing this Home.",
            L"",
            L"Type YES to confirm this single operation:",
        };
        for (std::size_t index = 0; index < std::size(lines); ++index)
            screen.text(box.x, box.y + static_cast<int>(index), lines[index], StyleDefault);
        screen.text(box.x, box.y + 7, confirmBuffer_, StyleBold);
        break;
    }
    case Modal::Repair: {
        const Box box = drawBox(screen, L"Repair Launch", 62, 8);
        const std::wstring lines[] = {
            L"Stops the shared Codex background server once and may",
            L"interrupt other clients' tasks on the same Home.",
            L"The backend proxy block is prepared before the stop.",
            L"",
            L"Type YES to confirm this single operation:",
        };
        for (std::size_t index = 0; index < std::size(lines); ++index)
            screen.text(box.x, box.y + static_cast<int>(index), lines[index], StyleDefault);
        screen.text(box.x, box.y + 6, confirmBuffer_, StyleBold);
        break;
    }
    case Modal::ErrorResult: {
        const Box box = drawBox(screen, L"Operation Failed", 62, 7);
        screen.text(box.x, box.y, widen(outcomeCode_), StyleRed | StyleBold);
        std::wstring message;
        appendTruncated(message, widen(outcomeMessage_), box.width);
        screen.text(box.x, box.y + 1, message, StyleDefault);
        screen.text(box.x, box.y + 3, L"C copy diagnostic \x00b7 Esc close", StyleDim);
        break;
    }
    case Modal::LaunchResult: {
        const Box box = drawBox(screen, L"Launch Submitted", 62, 5);
        screen.text(box.x, box.y, L"Desktop launch submitted; press R to refresh state.", StyleGreen);
        screen.text(box.x, box.y + 2, L"Esc close", StyleDim);
        break;
    }
    case Modal::Help: {
        const Box box = drawBox(screen, L"Help", 58, 12);
        const std::wstring lines[] = {
            L"Enter / L   Launch Desktop",
            L"E           Edit proxy host and port",
            L"B           Authorize or revoke the Codex backend proxy block",
            L"D           Repair launch (stop the shared daemon once)",
            L"R / F5      Refresh state",
            L"Ctrl+C      Cancel the foreground operation",
            L"Q           Quit (Guard never terminates Desktop)",
            L"",
            L"Guard delivers proxy settings to newly launched Desktop",
            L"processes only; closing Guard never closes Desktop.",
        };
        for (std::size_t index = 0; index < std::size(lines) && index < 11u; ++index)
            screen.text(box.x, box.y + static_cast<int>(index), lines[index], StyleDefault);
        break;
    }
    case Modal::QuitWhileBusy: {
        const Box box = drawBox(screen, L"Operation Active", 56, 5);
        screen.text(box.x, box.y, L"Cancel the running operation? Y confirms, Esc keeps it.", StyleYellow);
        break;
    }
    }
}

void TuiState::draw(Screen &screen) const
{
    screen.clear();
    drawMain(screen);
    drawModal(screen);
}

} // namespace cpg::tui
