#pragma once

#include "cpg/error.h"
#include "cpg/json.h"
#include "input.h"
#include "screen.h"

#include <string>

namespace cpg::tui {

enum class Modal { None, ProxyEditor, Consent, Repair, ErrorResult, Help, LaunchResult, QuitWhileBusy };

// Pure UI state machine over the session snapshot: it renders the view model
// and maps keys to controller actions. It never recomputes engine rules —
// capability flags and confirmation tokens are read from the snapshot.
class TuiState {
public:
    struct Action {
        enum class Kind {
            None, Refresh, Launch, OpenEditor, OpenConsent, OpenRepair, Quit, Cancel,
            SaveProxy, ConfirmConsent, ConfirmRepair, CopyDiagnostic,
        };
        Kind kind = Kind::None;
        std::wstring host;              // SaveProxy
        int port = 0;                   // SaveProxy
        bool enableConsent = false;     // ConfirmConsent
        std::string token;              // ConfirmConsent / ConfirmRepair
    };

    void setSnapshot(Json value) { snapshot_ = std::move(value); notice_.clear(); }
    void setBusy(bool busy, const std::string &label);
    void setOperationOutcome(const Error *error); // null = success receipt
    void setSpinnerPhase(int phase) { spinnerPhase_ = phase; }
    void setEditorSeed(const std::wstring &host, const std::wstring &port);
    void setConfirmHome(const std::wstring &home) { confirmHome_ = home; }
    void setDiagnostic(const std::string &text) { diagnostic_ = text; }
    // Transient one-shot feedback (e.g. "Diagnostic copied"); cleared by the
    // next snapshot, operation outcome or busy phase.
    void setNotice(const std::string &text) { notice_ = text; }

    Modal modal() const { return modal_; }
    void clearModal() { modal_ = Modal::None; }
    const Json &snapshot() const { return snapshot_; }
    // Single input event; returns the action for the controller to execute.
    Action key(const Key &key);
    // Compose the whole frame (main view + active modal + resize notice).
    void draw(Screen &screen) const;

    // Presentation helpers exposed for tests.
    static std::wstring blockedReason(const Json &snapshot);
    std::wstring editorError() const { return editorError_; }

private:
    Action mainKey(const Key &key);
    Action editorKey(const Key &key);
    Action confirmKey(const Key &key);
    void drawMain(Screen &screen) const;
    void drawModal(Screen &screen) const;

    Json snapshot_ = Json::object();
    bool busy_ = false;
    std::string busyLabel_;
    int spinnerPhase_ = 0;
    Modal modal_ = Modal::None;
    // Proxy editor state.
    struct {
        int field = 0; // 0 = host, 1 = port
        std::wstring host, port;
        std::size_t hostCursor = 0, portCursor = 0;
    } editor_;
    std::wstring editorError_;
    // YES-confirmation buffer for consent/repair modals.
    std::wstring confirmBuffer_;
    std::wstring confirmHome_;
    // Operation outcome rendering.
    bool hasOutcome_ = false;
    bool outcomeOk_ = false;
    std::string outcomeCode_, outcomeMessage_;
    std::string notice_;
    std::string diagnostic_;
};

} // namespace cpg::tui
