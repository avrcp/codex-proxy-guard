#include "tui_state.h"
#include "display_width.h"
#include "cpg/build_info.h"
#include "cpg/port.h"
#include "cpg/text.h"
#include <algorithm>
#include <cwctype>

namespace cpg::tui {
namespace {
std::wstring widen(const std::string &value) { return utf8ToWide(value); }
std::uint8_t stateStyle(const std::string &state)
{
    if (state == "ready" || state == "current" || state == "running" || state == "stopped"
        || state == "found" || state == "prepared") return StyleGreen;
    if (state == "unknown") return StyleDim;
    if (state == "pending" || state == "stale" || state == "not_authorized" || state == "repair_required") return StyleYellow;
    return StyleRed;
}
struct Box {
    int x, y, width, height;
    void text(Screen &screen, int row, std::wstring_view value, std::uint8_t style = StyleDefault) const {
        if (row < 0 || row >= height || width <= 0) return;
        std::wstring clipped;
        appendTruncated(clipped, value, width);
        screen.text(x, y + row, clipped, style);
    }
};
Box drawBox(Screen &screen, const std::wstring &title, int width, int height)
{
    width = std::max(0, std::min(width, screen.width() - 2));
    height = std::max(0, std::min(height, screen.height() - 2));
    if (width < 4 || height < 3) return {0, 0, 0, 0};
    const int x = (screen.width() - width) / 2, y = (screen.height() - height) / 2;
    for (int row = 0; row < height; ++row) {
        std::wstring line(static_cast<std::size_t>(width), (row == 0 || row == height - 1) ? L'─' : L' ');
        line.front() = row == 0 ? L'╭' : row == height - 1 ? L'╰' : L'│';
        line.back() = row == 0 ? L'╮' : row == height - 1 ? L'╯' : L'│';
        screen.text(x, y + row, line, StyleDim);
    }
    std::wstring clipped;
    appendTruncated(clipped, L" " + title + L" ", width - 4);
    screen.text(x + 2, y, clipped, StyleBold);
    return {x + 2, y + 1, width - 4, height - 2};
}
std::vector<std::wstring> wrap(std::wstring_view text, int width)
{
    std::vector<std::wstring> lines(1);
    if (width <= 0) return lines;
    int used = 0;
    for (std::size_t offset = 0; offset < text.size();) {
        const auto cluster = nextDisplayCluster(text, offset);
        offset += cluster.consumed;
        if (cluster.text == L"\n") { lines.emplace_back(); used = 0; continue; }
        if (used + cluster.width > width) { lines.emplace_back(); used = 0; }
        lines.back() += cluster.text;
        used += cluster.width;
    }
    return lines;
}
void scrollText(Screen &screen, const Box &box, std::wstring_view text, int first, int count, std::size_t offset)
{
    const auto lines = wrap(text, box.width);
    const auto start = std::min(offset, lines.size() > static_cast<std::size_t>(std::max(0, count))
        ? lines.size() - static_cast<std::size_t>(std::max(0, count)) : 0);
    for (int row = 0; row < count && start + row < lines.size(); ++row)
        box.text(screen, first + row, lines[start + row]);
}
std::size_t previousCluster(const std::wstring &text, std::size_t cursor)
{
    std::size_t previous = 0;
    for (std::size_t offset = 0; offset < cursor;) { previous = offset; offset += nextDisplayCluster(text, offset).consumed; }
    return previous;
}
}
void TuiState::setBusy(bool busy, const std::string &label)
{
    busy_ = busy;
    busyLabel_ = label;
}
void TuiState::setOperationOutcome(const Error *error)
{
    notice_.clear(); scroll_ = 0; outcomeFacts_.clear(); outcomeWarning_ = false;
    outcomeCode_ = error ? error->code : "";
    outcomeMessage_ = error ? publicText(error->message, 4096) : "Operation completed.";
    modal_ = error ? Modal::ErrorResult : Modal::LaunchResult;
}
void TuiState::setLaunchOutcome(const LaunchOutcome &outcome)
{
    setOperationOutcome(outcome.error ? &*outcome.error : nullptr);
    outcomeFacts_ = "Activation: " + outcome.activationState + "\nBackend config: " + outcome.backendProxyConfig
        + "\nDaemon: " + outcome.daemonPreparation;
    if (outcome.receipt) {
        const auto &receipt = *outcome.receipt;
        outcomeFacts_ += "\nPID: " + (receipt.pid ? std::to_string(*receipt.pid) : "unknown")
            + "\nInstance: " + receipt.instance + "\nProxy delivery: " + receipt.proxyDelivery;
        outcomeWarning_ = receipt.instance != "created";
        if (receipt.instance == "reused") outcomeMessage_ += " An existing instance was returned. Adoption of this proxy is unconfirmed.";
        else if (receipt.instance == "unknown") outcomeMessage_ += " Instance creation is unknown. Refresh status; do not automatically retry.";
        else if (!outcome.error) outcomeMessage_ = "Desktop activation returned a new instance.";
    }
    if (outcome.exceededBudget) outcomeFacts_ += "\nSession budget exceeded; returned facts are preserved.";
    outcomeFacts_ += "\nPrepared configuration is not a network verification.";
}
void TuiState::beginSave(std::int64_t operationId) { savingOperation_ = operationId; editorError_.clear(); }
void TuiState::applyMutationOutcome(const MutationOutcome &outcome)
{
    if (!savingOperation_ || *savingOperation_ != outcome.operationId) return;
    savingOperation_.reset();
    if (outcome.committed) {
        modal_ = Modal::None;
        setNotice(outcome.refreshError ? "Saved. Status refresh failed; R retries reading only." : "Proxy saved.");
    } else {
        modal_ = Modal::ProxyEditor;
        editorError_ = widen(outcome.mutationError ? publicText(outcome.mutationError->message, 1024) : "Save did not complete. Your draft is retained.");
        if (outcome.mutationError && outcome.mutationError->field == "proxy.host") editor_.field = 0;
        else if (outcome.mutationError && outcome.mutationError->field == "proxy.port") editor_.field = 1;
    }
}
void TuiState::setEditorSeed(const std::wstring &host, const std::wstring &port)
{
    editor_.field = 0; editor_.host = host; editor_.port = port;
    editor_.hostCursor = host.size(); editor_.portCursor = port.size();
    editorError_.clear(); savingOperation_.reset();
}
void TuiState::setConsentProposal(const ConsentProposal &proposal) { consent_ = proposal; confirmBuffer_.clear(); scroll_ = 0; }
void TuiState::setRepairProposal(const RepairProposal &proposal) { repair_ = proposal; confirmBuffer_.clear(); scroll_ = 0; }
TuiState::Action TuiState::key(const Key &key)
{
    if (key.control && (key.unicode == 0x03 || key.virtualKey == VK_CANCEL)) {
        Action action; if (busy_) action.kind = Action::Kind::Cancel; return action;
    }
    if (modal_ == Modal::QuitWhileBusy) {
        Action action;
        if (key.unicode == L'Y' || key.unicode == L'y') { action.kind = Action::Kind::CancelAndQuit; modal_ = Modal::None; }
        else if (key.virtualKey == VK_ESCAPE || key.virtualKey == VK_RETURN || key.unicode == L'n' || key.unicode == L'N') modal_ = Modal::None;
        return action;
    }
    if (busy_) {
        if (key.unicode == L'q' || key.unicode == L'Q') modal_ = Modal::QuitWhileBusy;
        return {};
    }
    switch (modal_) {
    case Modal::None: return mainKey(key);
    case Modal::ProxyEditor: return editorKey(key);
    case Modal::Consent: case Modal::Repair: return confirmKey(key);
    case Modal::ErrorResult: case Modal::LaunchResult:
        if (key.virtualKey == VK_UP) { if (scroll_) --scroll_; return {}; }
        if (key.virtualKey == VK_DOWN) { ++scroll_; return {}; }
        if (key.unicode == L'c' || key.unicode == L'C') { Action action; action.kind = Action::Kind::CopyDiagnostic; return action; }
        if (key.unicode == L'r' || key.unicode == L'R' || key.virtualKey == VK_F5) { Action action; action.kind = Action::Kind::Refresh; return action; }
        if (key.unicode == L'e' || key.unicode == L'E' || key.unicode == L'b' || key.unicode == L'B') return mainKey(key);
        if (key.virtualKey == VK_ESCAPE || key.virtualKey == VK_RETURN) modal_ = Modal::None;
        return {};
    case Modal::Help:
        if (key.virtualKey == VK_ESCAPE || key.virtualKey == VK_RETURN || key.virtualKey == VK_F1 || key.unicode == L'?') modal_ = Modal::None;
        return {};
    case Modal::QuitWhileBusy: return {};
    }
    return {};
}
TuiState::Action TuiState::mainKey(const Key &key)
{
    Action action;
    if (key.virtualKey == VK_RETURN || key.virtualKey == 'L') {
        if (snapshot_.actions.canLaunch) action.kind = Action::Kind::Launch;
        else setNotice(snapshot_.actions.blockedReason.empty() ? "Launch unavailable. R refreshes status." : snapshot_.actions.blockedReason);
        return action;
    }
    if (key.virtualKey == VK_F5) { if (snapshot_.actions.canRefresh) action.kind = Action::Kind::Refresh; return action; }
    switch (std::towlower(key.unicode)) {
    case L'r': if (snapshot_.actions.canRefresh) action.kind = Action::Kind::Refresh; return action;
    case L'e':
        if (snapshot_.actions.canEditProxy) { modal_ = Modal::ProxyEditor; action.kind = Action::Kind::OpenEditor; }
        else setNotice("Proxy editing unavailable - press R to refresh.");
        return action;
    case L'b':
        if ((snapshot_.actions.canAuthorizeBackendProxy || snapshot_.actions.canRevokeBackendProxy) && snapshot_.consent) {
            modal_ = Modal::Consent; setConsentProposal(*snapshot_.consent); action.kind = Action::Kind::OpenConsent;
        } else setNotice("Backend authorization unavailable - press R to refresh.");
        return action;
    case L'd':
        if (snapshot_.actions.canRepair && snapshot_.repair) { modal_ = Modal::Repair; setRepairProposal(*snapshot_.repair); action.kind = Action::Kind::OpenRepair; }
        else setNotice("Repair launch unavailable - press R to refresh.");
        return action;
    case L'q': action.kind = Action::Kind::Quit; return action;
    default: if (key.unicode == L'?' || key.virtualKey == VK_F1) modal_ = Modal::Help; return {};
    }
}
TuiState::Action TuiState::editorKey(const Key &key)
{
    if (key.virtualKey == VK_ESCAPE) { modal_ = Modal::None; return {}; }
    if (key.virtualKey == VK_TAB) { editor_.field = 1 - editor_.field; return {}; }
    if (key.virtualKey == VK_RETURN) {
        try { Action action; action.port = parsePortStrict(wideToUtf8(editor_.port)); action.host = editor_.host; action.kind = Action::Kind::SaveProxy; return action; }
        catch (const Error &) { editor_.field = 1; editorError_ = L"Port must be a number between 1 and 65535."; return {}; }
    }
    auto &value = editor_.field == 0 ? editor_.host : editor_.port;
    auto &cursor = editor_.field == 0 ? editor_.hostCursor : editor_.portCursor;
    if (key.virtualKey == VK_LEFT && cursor > 0) { cursor = previousCluster(value, cursor); return {}; }
    if (key.virtualKey == VK_RIGHT && cursor < value.size()) { cursor += nextDisplayCluster(value, cursor).consumed; return {}; }
    if (key.virtualKey == VK_HOME) { cursor = 0; return {}; }
    if (key.virtualKey == VK_END) { cursor = value.size(); return {}; }
    if (key.virtualKey == VK_BACK && cursor > 0) { const auto previous = previousCluster(value, cursor); value.erase(previous, cursor - previous); cursor = previous; return {}; }
    if (key.virtualKey == VK_DELETE && cursor < value.size()) { value.erase(cursor, nextDisplayCluster(value, cursor).consumed); return {}; }
    if (key.unicode >= 0x20 && value.size() < (editor_.field == 0 ? 255u : 5u)) {
        if (editor_.field == 1 && (key.unicode < L'0' || key.unicode > L'9')) return {};
        value.insert(cursor++, 1, key.unicode); editorError_.clear();
    }
    return {};
}
TuiState::Action TuiState::confirmKey(const Key &key)
{
    Action action;
    if (key.virtualKey == VK_ESCAPE) { modal_ = Modal::None; confirmBuffer_.clear(); return {}; }
    if (key.virtualKey == VK_UP) { if (scroll_) --scroll_; return {}; }
    if (key.virtualKey == VK_DOWN) { ++scroll_; return {}; }
    if (key.virtualKey == VK_BACK && !confirmBuffer_.empty()) { confirmBuffer_.pop_back(); return {}; }
    if (key.virtualKey == VK_RETURN) {
        if (confirmBuffer_ == L"YES") {
            if (modal_ == Modal::Repair && repair_) { action.kind = Action::Kind::ConfirmRepair; action.token = repair_->token; }
            else if (modal_ == Modal::Consent && consent_ && !consent_->home.empty()) { action.kind = Action::Kind::ConfirmConsent; action.enableConsent = consent_->enabled; action.token = consent_->token; }
            if (action.kind != Action::Kind::None) modal_ = Modal::None;
        }
        confirmBuffer_.clear(); return action;
    }
    if (key.unicode >= 0x20 && key.unicode < 0x7f && confirmBuffer_.size() < 8) confirmBuffer_.push_back(static_cast<wchar_t>(std::towupper(key.unicode)));
    return {};
}
std::wstring TuiState::blockedReason(const SessionView &snapshot)
{
    return widen(snapshot.actions.blockedReason.empty() ? "Status unknown - press R to refresh." : snapshot.actions.blockedReason);
}
void TuiState::drawMain(Screen &screen) const
{
    const auto &desktop = snapshot_.desktop;
    screen.text(0, 0, L"CODEX PROXY GUARD", StyleBold);
    std::wstring badge; std::uint8_t style = StyleDim;
    if (busy_) { badge = std::wstring(L"● ") + L"|/-\\"[spinnerPhase_ % 4] + L" " + widen(busyLabel_); style = StyleYellow; }
    else if (snapshot_.process.state == "running") { badge = L"● RUNNING"; style = StyleGreen; }
    else if (snapshot_.actions.canLaunch) { badge = L"● READY"; style = StyleGreen; }
    else { badge = L"● BLOCKED"; style = StyleYellow; }
    std::wstring clipped; appendTruncated(clipped, badge, screen.width() - 19);
    screen.text(std::max(19, screen.width() - displayWidth(clipped)), 0, clipped, style);
    screen.text(0, 1, L"Local proxy launcher", StyleDim); screen.rule(2, StyleDim);
    const auto row = [&](int index, const std::wstring &label, const std::wstring &value, std::uint8_t color) {
        if (3 + index >= screen.height() - 3) return;
        screen.text(1, 3 + index, label, StyleDim); screen.text(13, 3 + index, value, color);
    };
    row(0, L"Proxy", widen(snapshot_.proxy.url), StyleDefault);
    row(1, L"Launch", widen(snapshot_.launchMethod), StyleDefault);
    row(2, L"App", widen(desktop.displayName + " · " + desktop.packageVersion + " · " + desktop.architecture), StyleDefault);
    row(3, L"Entry", widen(desktop.applicationId.value_or("-") + " · " + desktop.runtimeKind.value_or("-")), StyleDefault);
    row(4, L"Process", widen(snapshot_.process.state), stateStyle(snapshot_.process.state));
    row(5, L"Coverage", widen(snapshot_.coverage.state) + (snapshot_.coverage.authorizedHome
        ? L" - " + snapshot_.coverage.authorizedHome->wstring() : L""), stateStyle(snapshot_.coverage.state));
    screen.rule(screen.height() - 3, StyleDim);
    if (busy_) screen.text(1, screen.height() - 2, L"Ctrl+C Cancel · Q Cancel and quit · " + widen(busyLabel_), StyleYellow);
    else if (!notice_.empty()) screen.text(1, screen.height() - 2, widen(notice_), StyleYellow);
    else if (snapshot_.actions.canLaunch) screen.text(1, screen.height() - 2, L"[ Enter ] Launch " + widen(desktop.displayName), StyleBold);
    else screen.text(1, screen.height() - 2, blockedReason(snapshot_), StyleYellow);
    screen.text(1, screen.height() - 1, L"E Proxy  B Codex Proxy  D Repair  R Refresh  ? Help  Q Quit", StyleDim);
}
void TuiState::drawModal(Screen &screen) const
{
    if (modal_ == Modal::None) return;
    if (modal_ == Modal::ProxyEditor) {
        const Box box = drawBox(screen, L"Proxy Configuration", 62, 11);
        const auto field = [&](int row, const std::wstring &label, const std::wstring &value, std::size_t cursor, bool focused) {
            const int space = std::max(0, box.width - 9);
            std::size_t start = 0;
            while (start < cursor && displayWidth(std::wstring_view(value).substr(start, cursor - start)) >= space && space > 0)
                start += nextDisplayCluster(value, start).consumed;
            std::wstring visible; appendTruncated(visible, std::wstring_view(value).substr(start), space);
            box.text(screen, row, label + L" [" + visible + L"]", focused ? StyleBold : StyleDefault);
            if (focused && space > 0 && row + 1 < box.height) {
                const int caret = displayWidth(std::wstring_view(value).substr(start, cursor - start));
                screen.text(box.x + 7 + std::min(caret, space - 1), box.y + row + 1, L"^", StyleBold);
            }
        };
        field(0, L"Host ", editor_.host, editor_.hostCursor, editor_.field == 0);
        field(2, L"Port ", editor_.port, editor_.portCursor, editor_.field == 1);
        if (!editorError_.empty()) scrollText(screen, box, editorError_, 4, 2, 0);
        box.text(screen, box.height - 1, saving() ? L"Saving... Ctrl+C cancels; draft retained." : L"Tab switch · Enter save · Esc cancel", StyleDim);
        return;
    }
    if (modal_ == Modal::Consent || modal_ == Modal::Repair) {
        const bool consent = modal_ == Modal::Consent;
        const Box box = drawBox(screen, consent ? L"Backend Proxy Authorization" : L"Repair Launch", 68, 17);
        std::wstring message;
        if (consent && consent_) {
            message = consent_->enabled ? L"Authorize HTTP_PROXY/HTTPS_PROXY/NO_PROXY in this exact Home:\n" : L"Remove Guard's unchanged proxy block from this exact Home:\n";
            message += consent_->home.wstring();
            message += L"\nOnly Guard's marked block is managed. It affects later Codex processes sharing this Home.";
        } else message = L"Stop the shared Codex background server once. This may interrupt other clients' tasks on the same Home.\nThe backend proxy block is prepared before the stop.";
        scrollText(screen, box, message, 0, std::max(0, box.height - 4), scroll_);
        box.text(screen, box.height - 3, L"Up/Down scroll full scope · Esc cancel", StyleDim);
        box.text(screen, box.height - 2, L"Type YES to confirm this single operation:", StyleYellow);
        box.text(screen, box.height - 1, confirmBuffer_ + L"_", StyleBold);
        return;
    }
    if (modal_ == Modal::QuitWhileBusy) {
        const auto box = drawBox(screen, L"Operation Active", 64, 6);
        box.text(screen, 0, L"Cancel the current operation and exit after cleanup?", StyleYellow);
        box.text(screen, 2, L"Y confirms · Enter/Esc keeps running (default)", StyleDim); return;
    }
    if (modal_ == Modal::Help) {
        const auto box = drawBox(screen, L"Help", 68, 15);
        scrollText(screen, box, L"Enter / L   Launch Desktop\nE   Edit proxy host and port\nB   Authorize/revoke backend proxy block\nD   Repair launch (one shared daemon stop)\nR / F5   Refresh status\nCtrl+C   Cancel current operation\nQ   Quit (never terminates Desktop)\n\nClosing Guard never closes Desktop.", 0, box.height - 1, 0);
        box.text(screen, box.height - 1, L"Esc / Enter / ? closes help", StyleDim); return;
    }
    const auto box = drawBox(screen, modal_ == Modal::ErrorResult ? L"Operation Failed" : L"Operation Result", 68, 17);
    box.text(screen, 0, widen(outcomeCode_.empty() ? (outcomeWarning_ ? "Outcome needs attention" : "Operation completed") : outcomeCode_),
        modal_ == Modal::ErrorResult ? StyleRed : outcomeWarning_ ? StyleYellow : StyleGreen);
    scrollText(screen, box, widen(outcomeMessage_ + "\n" + outcomeFacts_), 2, std::max(0, box.height - 4), scroll_);
    box.text(screen, box.height - 1, L"Up/Down details · R refresh · C copy · Esc close", StyleDim);
}
void TuiState::draw(Screen &screen) const { screen.clear(); drawMain(screen); drawModal(screen); }
std::string TuiState::diagnosticText() const
{
    std::string text = std::string("Codex Proxy Guard diagnostic\nversion: ") + productVersion() + "\ncommit: " + buildCommit();
    if (!outcomeMessage_.empty())
        text += "\noperation.error.code: " + outcomeCode_ + "\noperation.message: " + outcomeMessage_;
    if (snapshot_.error)
        text += "\nstatus.error.code: " + snapshot_.error->code + "\nstatus.error.message: " + snapshot_.error->message;
    text += "\ndesktop: " + snapshot_.desktop.displayName + "\nentry: " + snapshot_.desktop.applicationId.value_or("-");
    text += "\ncoverage: " + snapshot_.coverage.state + "\n" + outcomeFacts_;
    std::string redacted;
    for (std::size_t start = 0; start < text.size();) {
        const auto end = text.find('\n', start);
        redacted += publicText(text.substr(start, end == std::string::npos ? end : end - start), 4096) + "\n";
        if (end == std::string::npos) break;
        start = end + 1;
    }
    return redacted;
}
} // namespace cpg::tui
