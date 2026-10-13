#pragma once
#include "session.h"
#include "input.h"
#include "screen.h"
#include <string>
#include <optional>
namespace cpg::tui {
enum class Modal { None, ProxyEditor, Consent, Repair, ErrorResult, Help, LaunchResult, QuitWhileBusy };
class TuiState {
public:
    struct Action {
        enum class Kind { None, Refresh, Launch, OpenEditor, OpenConsent, OpenRepair, Quit, Cancel,
            SaveProxy, ConfirmConsent, ConfirmRepair, CopyDiagnostic, CancelAndQuit };
        Kind kind = Kind::None;
        std::wstring host;
        int port = 0;
        bool enableConsent = false;
        std::string token;
    };
    void setSnapshot(SessionView value) { snapshot_ = std::move(value); notice_.clear(); }
    void setBusy(bool busy, const std::string &label);
    void setOperationOutcome(const Error *error);
    void setLaunchOutcome(const LaunchOutcome &outcome);
    void beginSave(std::int64_t operationId);
    void applyMutationOutcome(const MutationOutcome &outcome);
    void setSpinnerPhase(int phase) { spinnerPhase_ = phase; }
    void setEditorSeed(const std::wstring &host, const std::wstring &port);
    void setConsentProposal(const ConsentProposal &proposal);
    void setRepairProposal(const RepairProposal &proposal);
    const std::optional<ConsentProposal> &consentProposal() const { return consent_; }
    const std::optional<RepairProposal> &repairProposal() const { return repair_; }
    void setNotice(const std::string &text) { notice_ = text; }
    const std::string &notice() const { return notice_; }
    Modal modal() const { return modal_; }
    void clearModal() { modal_ = Modal::None; }
    const SessionView &snapshot() const { return snapshot_; }
    Action key(const Key &key);
    void draw(Screen &screen) const;
    static std::wstring blockedReason(const SessionView &snapshot);
    std::wstring editorError() const { return editorError_; }
    const std::wstring &editorHost() const { return editor_.host; }
    const std::wstring &editorPort() const { return editor_.port; }
    int editorField() const { return editor_.field; }
    bool saving() const { return savingOperation_.has_value(); }
    std::string diagnosticText() const;
private:
    Action mainKey(const Key &key);
    Action editorKey(const Key &key);
    Action confirmKey(const Key &key);
    void drawMain(Screen &screen) const;
    void drawModal(Screen &screen) const;
    SessionView snapshot_;
    bool busy_ = false;
    std::string busyLabel_;
    int spinnerPhase_ = 0;
    Modal modal_ = Modal::None;
    struct {
        int field = 0;
        std::wstring host, port;
        std::size_t hostCursor = 0, portCursor = 0;
    } editor_;
    std::wstring editorError_;
    std::optional<std::int64_t> savingOperation_;
    std::optional<ConsentProposal> consent_;
    std::optional<RepairProposal> repair_;
    std::wstring confirmBuffer_;
    std::size_t scroll_ = 0;
    std::string outcomeCode_, outcomeMessage_, outcomeFacts_;
    bool outcomeWarning_ = false;
    std::string notice_;
};
} // namespace cpg::tui
