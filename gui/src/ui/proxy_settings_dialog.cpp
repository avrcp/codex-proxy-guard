#include "proxy_settings_dialog.h"
#include "theme.h"
#include "controller/launcher_controller.h"

#include <QDialogButtonBox>
#include <QFormLayout>
#include <QLabel>
#include <QLineEdit>
#include <QSpinBox>
#include <QVBoxLayout>

namespace guard {
namespace {
QLabel *textLabel(const QString &text, QWidget *parent) {
    auto *label = new QLabel(text, parent);
    label->setTextFormat(Qt::PlainText);
    label->setWordWrap(true);
    return label;
}
}

ProxySettingsDialog::ProxySettingsDialog(LauncherController *controller, QWidget *parent)
    : QDialog(parent), controller_(controller)
{
    setWindowTitle("Proxy settings");
    setMinimumWidth(380);
    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(20, 20, 20, 20);
    layout->setSpacing(16);
    layout->addWidget(textLabel(
        "Use a local HTTP or Mixed proxy. The engine validates the address before saving; "
        "the dialog stays open with your input until the save is confirmed.", this));
    auto *form = new QFormLayout;
    const auto proxy = controller_->snapshot().value("proxy").toObject();
    host_ = new QLineEdit(proxy.value("host").toString("127.0.0.1"), this);
    host_->setMaxLength(255);
    host_->setObjectName("proxyHost");
    host_->setAccessibleName("Proxy host");
    port_ = new QSpinBox(this);
    port_->setRange(1, 65535);
    port_->setValue(proxy.value("port").toInt(10808));
    port_->setObjectName("proxyPort");
    port_->setAccessibleName("Proxy port");
    form->addRow("&Host", host_);
    form->addRow("&Port", port_);
    layout->addLayout(form);
    status_ = textLabel(QString(), this);
    status_->setWordWrap(true);
    status_->setTextInteractionFlags(Qt::TextSelectableByMouse | Qt::TextSelectableByKeyboard);
    layout->addWidget(status_);
    buttons_ = new QDialogButtonBox(QDialogButtonBox::Save | QDialogButtonBox::Cancel, this);
    connect(buttons_, &QDialogButtonBox::accepted, this, &ProxySettingsDialog::submit);
    connect(buttons_, &QDialogButtonBox::rejected, this, &QDialog::reject);
    layout->addWidget(buttons_);
    // Bound completion: the bridge fails bounded requests itself, so the Saving
    // state always resolves through one of these three signals.
    connect(controller_, &LauncherController::proxySaveSucceeded, this, &QDialog::accept);
    connect(controller_, &LauncherController::proxySaveFailed, this,
            [this](const QString &code, const QString &message) {
                restoreEditing();
                // The engine message leads; the code moves into the tooltip so
                // a rejected save is readable without decoding a token.
                setDynamicProperty(status_, "error", true);
                status_->setText(QString("%1 Your edits are preserved; adjust and Save again.").arg(message));
                status_->setToolTip(code);
            });
    connect(controller_, &LauncherController::proxySaveOutcomeUnknown, this, [this](const QString &reason) {
        state_ = State::OutcomeUnknown;
        setControlsEnabled(true);
        setDynamicProperty(status_, "error", true);
        status_->setToolTip(QString());
        status_->setText(QString(
            "%1 interrupted the save; whether it was written is unconfirmed. Do not resubmit "
            "blindly: reconnect, refresh, and verify the stored settings before deciding.")
                             .arg(reason));
    });
}

void ProxySettingsDialog::reset()
{
    if (state_ == State::Saving) return; // Never disturb an in-flight save by reopening.
    state_ = State::Editing;
    const auto proxy = controller_->snapshot().value("proxy").toObject();
    host_->setText(proxy.value("host").toString("127.0.0.1"));
    port_->setValue(proxy.value("port").toInt(10808));
    status_->clear();
    status_->setToolTip(QString());
    setDynamicProperty(status_, "error", false);
    setControlsEnabled(true);
}

void ProxySettingsDialog::reject()
{
    if (state_ == State::Saving) return; // Block Cancel, Esc, and the window X while saving.
    QDialog::reject();
}

void ProxySettingsDialog::submit()
{
    if (state_ != State::Editing) return; // One save at a time; Enter or double clicks do not duplicate.
    // Every branch below blocks the save, so each one must say why in the
    // error tone rather than leaving the dialog looking untouched.
    if (host_->text().trimmed().isEmpty()) {
        setDynamicProperty(status_, "error", true);
        status_->setToolTip(QString());
        status_->setText("Enter a proxy host. Guard accepts a local HTTP or Mixed proxy only.");
        return;
    }
    if (!controller_->setProxy(host_->text().trimmed(), port_->value())) {
        setDynamicProperty(status_, "error", true);
        status_->setToolTip(QString());
        status_->setText("The engine cannot accept this request now. Refresh, then try again.");
        return;
    }
    setSaving();
}

void ProxySettingsDialog::setSaving()
{
    state_ = State::Saving;
    setControlsEnabled(false);
    setDynamicProperty(status_, "error", false);
    status_->setToolTip(QString());
    status_->setText("Saving…");
}

void ProxySettingsDialog::restoreEditing()
{
    state_ = State::Editing;
    setControlsEnabled(true);
}

void ProxySettingsDialog::setControlsEnabled(bool enabled)
{
    host_->setEnabled(enabled);
    port_->setEnabled(enabled);
    buttons_->setEnabled(enabled);
}

} // namespace guard
