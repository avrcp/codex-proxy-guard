#pragma once

#include <QDialog>

class QDialogButtonBox;
class QLabel;
class QLineEdit;
class QSpinBox;

namespace guard {

class LauncherController;

// Proxy editor spanning the asynchronous engine save. Saving keeps the dialog
// open with the user's input; only the engine's set_proxy confirmation closes
// it. Failures return to editing in place, and an engine failure during the
// save reports an unconfirmed result instead of resubmitting.
class ProxySettingsDialog final : public QDialog {
    Q_OBJECT
public:
    ProxySettingsDialog(LauncherController *controller, QWidget *parent);
    // Re-arm for a fresh editing session from the current snapshot. Connections
    // were made once at construction, so reopening never duplicates them.
    void reset();

protected:
    void reject() override;

private:
    enum class State { Editing, Saving, OutcomeUnknown };
    void submit();
    void setSaving();
    void restoreEditing();
    void setControlsEnabled(bool enabled);
    LauncherController *controller_;
    QLineEdit *host_;
    QSpinBox *port_;
    QLabel *status_;
    QDialogButtonBox *buttons_;
    State state_ = State::Editing;
};

} // namespace guard
