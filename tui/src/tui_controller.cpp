#include "tui_controller.h"
#include "clipboard.h"
#include "cpg/text.h"
namespace cpg::tui {
void TuiController::synchronize()
{
    state_.setSnapshot(session_.view());
    state_.setBusy(session_.busy(), session_.cancellingAfterTimeout() ? "Cancelling timed-out operation"
        : exitRequested_ ? "Cleaning up before exit" : session_.launching() ? "Launching Desktop" : "Updating");
}
void TuiController::refresh() { session_.requestRefresh(); synchronize(); }
void TuiController::cancel() { if (session_.busy()) session_.cancel(); }
void TuiController::requestClose() { exitRequested_ = true; cancel(); synchronize(); }
void TuiController::handleKey(const Key &key)
{
    const auto action = state_.key(key);
    try {
        switch (action.kind) {
        case TuiState::Action::Kind::Quit: exitRequested_ = true; break;
        case TuiState::Action::Kind::CancelAndQuit: requestClose(); break;
        case TuiState::Action::Kind::Cancel: cancel(); break;
        case TuiState::Action::Kind::Refresh: refresh(); break;
        case TuiState::Action::Kind::Launch: session_.requestLaunch(false, ""); break;
        case TuiState::Action::Kind::OpenEditor:
            state_.setEditorSeed(utf8ToWide(session_.view().proxy.host), std::to_wstring(session_.view().proxy.port)); break;
        case TuiState::Action::Kind::OpenConsent: case TuiState::Action::Kind::OpenRepair: break;
        case TuiState::Action::Kind::SaveProxy:
            session_.requestSetProxy(wideToUtf8(action.host), action.port);
            state_.beginSave(session_.operationId()); break;
        case TuiState::Action::Kind::ConfirmConsent:
            if (state_.consentProposal()) session_.requestSetBackendConsent(*state_.consentProposal()); break;
        case TuiState::Action::Kind::ConfirmRepair:
            if (state_.repairProposal()) session_.requestLaunch(*state_.repairProposal()); break;
        case TuiState::Action::Kind::CopyDiagnostic: {
            const auto diagnostic = utf8ToWide(state_.diagnosticText());
            const bool copied = copyDiagnostic_ ? copyDiagnostic_(diagnostic) : copyDiagnosticToClipboard(diagnostic);
            state_.setNotice(copied ? "Diagnostic copied."
                : "CLIPBOARD_FAILED: Cannot copy the diagnostic; the operation result is retained.");
            break;
        }
        case TuiState::Action::Kind::None: break;
        }
    } catch (const Error &error) {
        // A refused proposal can burn its token before throwing. Publish that
        // authoritative state immediately, while retaining the result/draft.
        state_.setSnapshot(session_.view());
        if (action.kind == TuiState::Action::Kind::SaveProxy) {
            const auto id = session_.operationId(); state_.beginSave(id);
            MutationOutcome outcome; outcome.operationId = id; outcome.mutationError = error;
            state_.applyMutationOutcome(outcome);
        } else state_.setOperationOutcome(&error);
    }
    // Preserve local notices while refreshing only the busy gate. Snapshot
    // changes are applied from poll or before requests in refresh().
    state_.setBusy(session_.busy(), session_.launching() ? "Launching Desktop" : "Updating");
}
void TuiController::poll()
{
    const auto event = session_.poll();
    const auto view = session_.view();
    if (event != EngineSession::Event::None) state_.setSnapshot(view);
    if (event == EngineSession::Event::SnapshotArrived && view.error) {
        if (state_.modal() == Modal::None && !session_.launchOutcome())
            state_.setOperationOutcome(&*view.error);
        else if (session_.launchOutcome()) state_.setNotice("Status refresh failed; launch result is retained.");
    } else if (event == EngineSession::Event::MutationFinished && session_.mutationOutcome()) {
        const auto &outcome = *session_.mutationOutcome();
        if (state_.saving()) state_.applyMutationOutcome(outcome);
        else if (outcome.mutationError) state_.setOperationOutcome(&*outcome.mutationError);
        else state_.setNotice(outcome.refreshError ? "Saved. Status refresh failed; R retries reading only." : "Authorization updated.");
    } else if (event == EngineSession::Event::MutationFailed) {
        const auto error = session_.mutationError();
        // A read failure must not replace the launch receipt panel.
        if (error && !session_.launchOutcome()) state_.setOperationOutcome(&*error);
        else if (error) state_.setNotice("Status refresh failed; launch result is retained.");
    } else if (event == EngineSession::Event::OperationFinished && session_.launchOutcome()) {
        const auto &outcome = *session_.launchOutcome(); state_.setLaunchOutcome(outcome);
        if (!exitRequested_ && outcome.needsRefresh && refreshedLaunch_ != outcome.operationId) {
            refreshedLaunch_ = outcome.operationId;
            session_.requestRefresh();
        }
    }
    state_.setBusy(session_.busy(), session_.cancellingAfterTimeout() ? "Cancelling timed-out operation"
        : exitRequested_ ? "Cleaning up before exit" : session_.launching() ? "Launching Desktop" : "Updating");
}
} // namespace cpg::tui
