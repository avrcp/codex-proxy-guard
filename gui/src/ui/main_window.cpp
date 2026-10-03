#include "main_window.h"
#include "theme.h"
#include "widgets/info_row.h"
#include <QApplication>
#include <QAbstractButton>
#include <QClipboard>
#include <QCloseEvent>
#include <QDialog>
#include <QDialogButtonBox>
#include <QFormLayout>
#include <QFrame>
#include <QHBoxLayout>
#include <QLabel>
#include <QJsonDocument>
#include <QLineEdit>
#include <QMessageBox>
#include <QProgressBar>
#include <QPushButton>
#include <QShortcut>
#include <QSpinBox>
#include <QScrollArea>
#include <QStyleHints>
#include <QVBoxLayout>

namespace guard {
namespace {
QLabel *textLabel(const QString &text, QWidget *parent) {
    auto *label = new QLabel(text, parent);
    label->setTextFormat(Qt::PlainText);
    label->setWordWrap(true);
    return label;
}
bool confirm(QWidget *parent, const QString &title, const QString &body, const QString &action, const QString &home = {}) {
    QDialog dialog(parent);
    dialog.setWindowTitle(title);
    dialog.setMinimumWidth(430);
    auto *layout = new QVBoxLayout(&dialog);
    layout->setSpacing(16);
    layout->setContentsMargins(20, 20, 20, 20);
    layout->addWidget(textLabel(body, &dialog));
    if (!home.isEmpty()) {
        auto *path = new QLineEdit(home, &dialog);
        path->setReadOnly(true);
        path->setCursorPosition(0);
        path->setToolTip(home);
        path->setAccessibleName("Exact Codex Home");
        layout->addWidget(path);
    }
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Cancel, &dialog);
    auto *accept = buttons->addButton(action, QDialogButtonBox::AcceptRole);
    accept->setAutoDefault(false);
    buttons->button(QDialogButtonBox::Cancel)->setDefault(true);
    QObject::connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    layout->addWidget(buttons);
    return dialog.exec() == QDialog::Accepted;
}
QString knownText(const QString &value, const QMap<QString, QString> &labels) {
    return labels.value(value, "Unknown");
}
}
MainWindow::MainWindow(LauncherController *controller, QWidget *parent)
    : QMainWindow(parent), controller_(controller) {
    setWindowTitle("Codex Proxy Guard");
    resize(520, 430);
    setMinimumSize(480, 400);
    auto *central = new QWidget(this);
    auto *layout = new QVBoxLayout(central);
    layout->setContentsMargins(20, 18, 20, 16);
    layout->setSpacing(10);
    auto *header = new QHBoxLayout;
    auto *title = textLabel("Codex Proxy Guard", central);
    title->setObjectName("heading");
    badge_ = textLabel("CONNECTING", central);
    badge_->setObjectName("badge");
    badge_->setWordWrap(false);
    header->addWidget(title, 1);
    header->addWidget(badge_);
    layout->addLayout(header);
    auto *subtitle = textLabel("Launch ChatGPT Desktop through your local proxy", central);
    subtitle->setProperty("muted", true);
    layout->addWidget(subtitle);
    auto *rows = new QFrame(central);
    rows->setObjectName("rows");
    auto *rowLayout = new QVBoxLayout(rows);
    rowLayout->setSpacing(0); rowLayout->setContentsMargins(0, 5, 0, 5);
    edit_ = new QPushButton("Edit", rows);
    edit_->setAccessibleName("Edit proxy settings");
    proxy_ = new InfoRow("Proxy", rows, edit_);
    launchMethod_ = new InfoRow("Launch", rows);
    app_ = new InfoRow("App", rows);
    entry_ = new InfoRow("Entry", rows);
    process_ = new InfoRow("Process", rows);
    coverage_ = new InfoRow("Coverage", rows);
    for (auto *row : {proxy_, launchMethod_, app_, entry_, process_, coverage_}) rowLayout->addWidget(row);
    layout->addWidget(rows);
    progress_ = new QProgressBar(central);
    progress_->setRange(0, 0); progress_->setTextVisible(false);
    progress_->setAccessibleName("Engine operation in progress");
    layout->addWidget(progress_);
    launch_ = new QPushButton("Launch ChatGPT Desktop", central);
    launch_->setObjectName("launch");
    layout->addWidget(launch_);
    auto *actions = new QHBoxLayout;
    backend_ = new QPushButton("Codex proxy", central);
    repair_ = new QPushButton("Repair", central);
    refresh_ = new QPushButton("Refresh", central);
    cancel_ = new QPushButton("Cancel", central);
    restart_ = new QPushButton("Restart Engine", central);
    for (auto *button : {backend_, repair_, refresh_, cancel_, restart_}) actions->addWidget(button);
    layout->addLayout(actions);
    auto *footer = new QHBoxLayout;
    status_ = textLabel("Connecting to engine…", central);
    status_->setProperty("muted", true);
    status_->setSizePolicy(QSizePolicy::Ignored, QSizePolicy::Preferred);
    details_ = new QPushButton("Details", central);
    auto *help = new QPushButton("Help", central);
    footer->addWidget(status_, 1); footer->addWidget(details_); footer->addWidget(help);
    layout->addLayout(footer);
    layout->addStretch();
    setCentralWidget(central);
    connect(controller_, &LauncherController::changed, this, &MainWindow::render);
    connect(controller_, &LauncherController::closed, this, [this] {
        if (controller_->errorCode() == "ENGINE_SHUTDOWN_TIMEOUT")
            QMessageBox::warning(this, "Engine cleanup timed out", controller_->errorMessage());
        closeReady_ = true; close();
    }, Qt::QueuedConnection);
    connect(QGuiApplication::styleHints(), &QStyleHints::colorSchemeChanged, this, [this] { render(); });
    connect(launch_, &QPushButton::clicked, this, [this] { controller_->launch(); });
    connect(edit_, &QPushButton::clicked, this, &MainWindow::proxyDialog);
    connect(backend_, &QPushButton::clicked, this, &MainWindow::backendDialog);
    connect(repair_, &QPushButton::clicked, this, &MainWindow::repairDialog);
    connect(refresh_, &QPushButton::clicked, controller_, &LauncherController::refresh);
    connect(cancel_, &QPushButton::clicked, controller_, &LauncherController::cancel);
    connect(restart_, &QPushButton::clicked, controller_, &LauncherController::start);
    connect(details_, &QPushButton::clicked, this, &MainWindow::errorDialog);
    connect(help, &QPushButton::clicked, this, &MainWindow::aboutDialog);
    const auto shortcut = [this](const QKeySequence &key, auto callback) {
        auto *binding = new QShortcut(key, this);
        binding->setContext(Qt::WindowShortcut);
        connect(binding, &QShortcut::activated, this, callback);
    };
    const auto activateFocused = [this] {
        if (auto *button = qobject_cast<QAbstractButton *>(QApplication::focusWidget())) button->click();
        else controller_->launch();
    };
    shortcut(QKeySequence(Qt::Key_Return), activateFocused);
    shortcut(QKeySequence(Qt::Key_Enter), activateFocused);
    shortcut(QKeySequence("Ctrl+,"), [this] { proxyDialog(); });
    shortcut(QKeySequence("C"), [this] { proxyDialog(); });
    shortcut(QKeySequence("B"), [this] { backendDialog(); });
    shortcut(QKeySequence("D"), [this] { repairDialog(); });
    shortcut(QKeySequence("R"), [this] { controller_->refresh(); });
    shortcut(QKeySequence(Qt::Key_F5), [this] { controller_->refresh(); });
    shortcut(QKeySequence(Qt::Key_F1), [this] { aboutDialog(); });
    shortcut(QKeySequence("?"), [this] { aboutDialog(); });
    render();
}
void MainWindow::render() {
    const auto &s = controller_->snapshot();
    const auto t = ThemeTokens::system();
    const auto desktop = s.value("desktop").toObject();
    const auto process = s.value("process").toObject();
    const auto coverage = s.value("coverage").toObject();
    const auto state = coverage.value("state").toString();
    proxy_->setValue(s.value("proxy").toObject().value("url").toString("Unknown"));
    launchMethod_->setValue(knownText(s.value("launch").toObject().value("method").toString(), {{"appmodel_activation", "Windows application activation"}, {"native_process", "Process environment"}}));
    app_->setValue(desktop.value("display_name").toString("Not discovered") + "  " + desktop.value("package_version").toString());
    entry_->setValue(desktop.value("manifest_executable").toString("Unknown"));
    const auto processState = process.value("state").toString();
    process_->setValue(knownText(processState, {{"running", "Running"}, {"stopped", "Stopped"}}));
    process_->setTone(processState == "running" ? t.success : t.secondary);
    coverage_->setValue(knownText(state, {{"not_authorized", "Setup required"}, {"not_applicable", "Process environment"}, {"pending", "Pending next launch"}, {"current", "Configuration current"}, {"stale", "Changed · launch to update"}, {"conflict", "Configuration conflict"}, {"invalid", "Invalid configuration"}, {"unavailable", "Unavailable"}, {"unsupported", "Cannot auto-edit · fix .env manually"}}));
    const bool coverageError = state == "conflict" || state == "invalid" || state == "unavailable";
    coverage_->setTone(state == "current" ? t.success : coverageError ? t.danger : state == "unknown" || state.isEmpty() ? t.secondary : t.warning);
    coverage_->setToolTip(state == "unsupported"
        ? "The Codex Home .env uses multi-line or unterminated quoting. Guard refuses to edit it automatically; no network state is implied. Fix the file manually, then retry."
        : QString());
    launch_->setEnabled(controller_->allows("can_launch"));
    edit_->setEnabled(controller_->allows("can_edit_proxy"));
    backend_->setEnabled(controller_->allows("can_authorize_backend_proxy"));
    repair_->setEnabled(controller_->allows("can_repair"));
    refresh_->setEnabled(controller_->connected() && !controller_->busy() && !controller_->closing());
    cancel_->setVisible(controller_->operating());
    cancel_->setEnabled(!controller_->closing());
    restart_->setVisible(!controller_->connected());
    restart_->setEnabled(!controller_->busy() && !controller_->closing());
    progress_->setVisible(controller_->busy());
    launch_->setText(controller_->busy() ? "Working…" : processState == "running" ? "ChatGPT Desktop is running" : "Launch ChatGPT Desktop");
    const auto badge = !controller_->connected() ? "OFFLINE" : controller_->busy() ? "WORKING" : processState == "running" ? "RUNNING" : controller_->allows("can_launch") ? "READY" : "BLOCKED";
    badge_->setText(badge);
    badge_->setStyleSheet("color:" + (controller_->allows("can_launch") || processState == "running" ? t.success : t.secondary).name());
    const bool error = !controller_->errorCode().isEmpty();
    status_->setText(error ? controller_->errorCode().replace('_', ' ') : controller_->message());
    status_->setToolTip(error ? controller_->errorMessage() : controller_->message());
    details_->setVisible(error || !controller_->receipt().isEmpty());
}
void MainWindow::proxyDialog() {
    if (!controller_->allows("can_edit_proxy")) return;
    QDialog dialog(this); dialog.setWindowTitle("Proxy settings"); dialog.setMinimumWidth(380);
    auto *layout = new QVBoxLayout(&dialog);
    layout->setContentsMargins(20,20,20,20); layout->setSpacing(16);
    layout->addWidget(textLabel("Use a local HTTP or Mixed proxy. The engine validates the address before saving.", &dialog));
    auto *form = new QFormLayout;
    const auto proxy = controller_->snapshot().value("proxy").toObject();
    auto *host = new QLineEdit(proxy.value("host").toString("127.0.0.1"), &dialog);
    host->setMaxLength(255); host->setAccessibleName("Proxy host");
    auto *port = new QSpinBox(&dialog); port->setRange(1,65535);
    port->setValue(proxy.value("port").toInt(10808)); port->setAccessibleName("Proxy port");
    form->addRow("&Host", host); form->addRow("&Port", port); layout->addLayout(form);
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Save | QDialogButtonBox::Cancel, &dialog);
    connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    layout->addWidget(buttons);
    if (dialog.exec() == QDialog::Accepted) controller_->setProxy(host->text(), port->value());
}
void MainWindow::backendDialog() {
    if (!controller_->allows("can_authorize_backend_proxy")) return;
    const auto proposal = controller_->snapshot().value("confirmations").toObject().value("backend_proxy").toObject();
    const auto token = proposal.value("token").toString();
    if (token.isEmpty()) return;
    const bool enable = proposal.value("enabled").toBool();
    const auto home = proposal.value("home").toString();
    if (confirm(this, enable ? "Authorize Codex backend proxy" : "Revoke Codex backend proxy",
        enable ? "Guard will manage only its marked HTTP_PROXY / HTTPS_PROXY / NO_PROXY block in this Codex Home.\n\nThis affects later Codex processes using the same Home. Preparation happens on the next launch; it is not a network check."
               : "Guard removes only its own unmodified proxy block. Other file content stays intact. If removal fails, the existing authorization remains available for retry.",
        enable ? "Authorize" : "Revoke", home)) controller_->setBackendConsent(enable, token);
}
void MainWindow::repairDialog() {
    if (!controller_->allows("can_repair")) return;
    const auto token = controller_->snapshot().value("confirmations").toObject().value("repair").toObject().value("token").toString();
    if (confirm(this, "Repair and launch", "This stops the shared Codex background server. Other CLI, IDE or remote tasks using the same Home may be interrupted.\n\nGuard prepares the authorized backend proxy configuration first. This confirmation applies to one launch only.", "Repair & Launch")) controller_->launch(true, token);
}
void MainWindow::errorDialog() {
    QDialog dialog(this); dialog.setWindowTitle("Action details"); dialog.setMinimumWidth(430);
    auto *layout = new QVBoxLayout(&dialog); layout->setSpacing(16);
    const auto receipt = controller_->receipt();
    const auto detailText = controller_->errorCode().isEmpty()
        ? QString("Launch: %1\nProxy delivery: %2\nBackend configuration: %3\nDaemon: %4\nPID: %5\n\nThese are engine observations, not network verification.")
            .arg(receipt.value("launch_method").toString(), receipt.value("proxy_delivery").toString(),
                 receipt.value("backend_proxy_config").toString(), receipt.value("daemon_preparation").toString(),
                 QString::number(receipt.value("pid").toInt()))
        : controller_->errorCode() + "\n\n" + controller_->errorMessage();
    auto *detail = textLabel(detailText, &dialog);
    detail->setTextInteractionFlags(Qt::TextSelectableByMouse | Qt::TextSelectableByKeyboard);
    auto *scroll = new QScrollArea(&dialog);
    scroll->setWidgetResizable(true);
    scroll->setFrameShape(QFrame::NoFrame);
    scroll->setWidget(detail);
    scroll->setMinimumSize(390, 150);
    layout->addWidget(scroll);
    dialog.resize(470, 300);
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Close, &dialog);
    auto *copy = buttons->addButton("Copy diagnostic", QDialogButtonBox::ActionRole);
    connect(copy, &QPushButton::clicked, &dialog, [this] {
        const auto s = controller_->snapshot();
        QApplication::clipboard()->setText(QStringLiteral("Codex Proxy Guard %1\nCommit: %2\nError: %3\nDesktop: %4\nCoverage: %5")
            .arg(CPG_PRODUCT_VERSION, CPG_BUILD_COMMIT, controller_->errorCode(), s.value("desktop").toObject().value("package_version").toString(), s.value("coverage").toObject().value("state").toString()));
    });
    const auto action = errorAction(controller_->errorCode());
    if (action != ErrorAction::None) {
        auto *resolve = buttons->addButton(action == ErrorAction::ProxySettings ? "Proxy settings" : action == ErrorAction::Refresh ? "Refresh" : "Codex proxy", QDialogButtonBox::AcceptRole);
        connect(resolve, &QPushButton::clicked, &dialog, &QDialog::accept);
    }
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject); layout->addWidget(buttons);
    if (dialog.exec() == QDialog::Accepted) {
        if (action == ErrorAction::ProxySettings) proxyDialog();
        else if (action == ErrorAction::Refresh) controller_->refresh();
        else if (action == ErrorAction::BackendProxy) backendDialog();
    }
}
void MainWindow::aboutDialog() {
    QMessageBox::about(this, "About Codex Proxy Guard", QStringLiteral("Codex Proxy Guard %1\nQt %2 · protocol 1\n\nEnter: Launch  ·  C / Ctrl+,: Proxy\nB: Codex proxy  ·  D: Repair\nR / F5: Refresh  ·  Esc: Close dialog\n\nProxy preparation is not network verification.\nClosing Guard never terminates Desktop.\n\nApplication: MIT · Qt: LGPLv3\nSee THIRD_PARTY_NOTICES.md and licenses in the portable folder.").arg(CPG_PRODUCT_VERSION, qVersion()));
}
void MainWindow::closeEvent(QCloseEvent *event) {
    if (closeReady_) { event->accept(); return; }
    event->ignore();
    if (controller_->closing()) return;
    if (controller_->busy() && !confirm(this, "Close Guard", "An operation is still running. Cancel it and exit? Guard will wait for bounded cleanup. Desktop will remain running if activation has already happened.", "Cancel operation & exit")) return;
    controller_->close();
}
}
