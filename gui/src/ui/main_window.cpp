#include "main_window.h"
#include "licenses_dialog.h"
#include "proxy_settings_dialog.h"
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
#include <QStringList>
#include <QStyleHints>
#include <QTimer>
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
// Shows the raw engine value when it has no friendly label. Collapsing an
// unrecognized state to "Unknown" hid the very thing a user or a support log
// needs; the raw token stays visible and the caller adds context.
QString knownText(const QString &value, const QMap<QString, QString> &labels) {
    if (value.isEmpty()) return QStringLiteral("Unknown");
    return labels.value(value, value);
}
// Explains a token knownText() chose to show verbatim, so it reads as "value
// Guard does not recognize" rather than as a real state.
QString explainUnknown(const QString &raw, const QMap<QString, QString> &labels) {
    return labels.contains(raw) || raw.isEmpty()
        ? QString()
        : QStringLiteral("Unrecognized engine state: \"%1\". Guard cannot interpret it.").arg(raw);
}
// Renders the registered shortcut table for the About dialog. Consecutive
// entries with the same action (one key each) merge into "Key / Key — Action",
// so a key never appears in the table without a matching binding.
QString formatShortcuts(const QVector<QPair<QString, QString>> &rows) {
    QStringList lines;
    for (int i = 0; i < rows.size(); ++i) {
        QStringList keys{rows[i].first};
        while (i + 1 < rows.size() && rows[i + 1].second == rows[i].second)
            keys << rows[++i].first;
        lines << QStringLiteral("%1 — %2").arg(keys.join(QStringLiteral(" / ")), rows[i].second);
    }
    return lines.join(QLatin1Char('\n'));
}
// Why the launch action is unavailable, in the user's terms. The engine already
// parses every field below (see Snapshot::fromJson), but the window never
// showed them, so a disabled Launch button was unexplained. Each clause mirrors
// exactly one condition of the engine's own safeToLaunch rule — nothing is
// invented here and nothing the engine gates on is left unstated.
QString launchBlockReason(const QJsonObject &snapshot, bool busy) {
    if (busy) return {};
    QStringList reasons;
    if (snapshot.value("config_readiness").toString() != "ready")
        reasons << QStringLiteral("proxy configuration is not ready; open Proxy to correct it");
    if (snapshot.value("desktop").toObject().value("state").toString() != "found")
        reasons << QStringLiteral("ChatGPT Desktop was not discovered on this system");
    if (snapshot.value("process").toObject().value("state").toString() == "running")
        reasons << QStringLiteral("ChatGPT Desktop is already running");
    const auto elevation = snapshot.value("elevation").toString();
    if (elevation == "elevated")
        reasons << QStringLiteral("Guard is running elevated; run it as a normal user");
    else if (elevation != "not_elevated")
        reasons << QStringLiteral("Guard could not verify it is not elevated");
    const auto method = snapshot.value("launch").toObject().value("method").toString();
    if (method != "appmodel_activation" && method != "native_process")
        reasons << QStringLiteral("the engine reported a launch method this Guard does not recognize");
    return reasons.join(QStringLiteral(" · "));
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
    // Small uppercase tracking reads as separate letters, not one blob; QSS has
    // no letter-spacing property, so the QFont carries it.
    auto badgeFont = badge_->font();
    badgeFont.setLetterSpacing(QFont::AbsoluteSpacing, 0.4);
    badge_->setFont(badgeFont);
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
    rows_ = rows;
    edit_ = new QPushButton("Edit", rows);
    edit_->setAccessibleName("Edit proxy settings");
    // The row-level Edit is reachable by mouse and by its accelerator (C /
    // Ctrl+,); keeping it out of the tab chain stops the first Tab stop from
    // landing on a secondary action instead of Launch.
    edit_->setFocusPolicy(Qt::NoFocus);
    proxy_ = new InfoRow("Proxy", rows, edit_);
    launchMethod_ = new InfoRow("Launch", rows);
    app_ = new InfoRow("App", rows);
    entry_ = new InfoRow("Entry", rows);
    process_ = new InfoRow("Process", rows);
    coverage_ = new InfoRow("Coverage", rows);
    for (auto *row : {proxy_, launchMethod_, app_, entry_, process_, coverage_}) rowLayout->addWidget(row);
    layout->addWidget(rows);
    // The progress bar is an overlay on the bottom edge of the information
    // card, not a layout row of its own: a hidden-then-shown widget in a
    // vertical flow moves every control below it, which made the window jump
    // when an operation began. As a positioned child of `rows` it never
    // participates in any layout.
    progress_ = new QProgressBar(rows);
    progress_->setRange(0, 0); progress_->setTextVisible(false);
    progress_->setAccessibleName("Engine operation in progress");
    progress_->setAttribute(Qt::WA_TransparentForMouseEvents);
    progress_->setGeometry(1, rows->height() - 4, rows->width() - 2, 3);
    progress_->setVisible(false);
    rows->installEventFilter(this);
    launch_ = new QPushButton("Launch ChatGPT Desktop", central);
    launch_->setObjectName("launch");
    layout->addWidget(launch_);
    auto *actions = new QHBoxLayout;
    actions->setSpacing(10);
    backend_ = new QPushButton("Codex proxy", central);
    repair_ = new QPushButton("Repair", central);
    refresh_ = new QPushButton("Refresh", central);
    cancel_ = new QPushButton("Cancel", central);
    restart_ = new QPushButton("Restart Engine", central);
    for (auto *button : {backend_, repair_, refresh_, cancel_, restart_}) actions->addWidget(button);
    // A stable row: buttons are disabled when unavailable instead of being
    // removed, so nothing shifts sideways when the engine state changes.
    actions->addStretch(1);
    layout->addLayout(actions);
    auto *footer = new QHBoxLayout;
    footer->setSpacing(10);
    status_ = new ElidedLabel(central);
    status_->setObjectName("status");
    status_->setRowLabel(QStringLiteral("Status"));
    status_->setProperty("muted", true);
    status_->setMinimumWidth(120);
    details_ = new QPushButton("Details", central);
    auto *help = new QPushButton("Help", central);
    help->setAccessibleName("About Codex Proxy Guard");
    footer->addWidget(status_, 1); footer->addWidget(details_); footer->addWidget(help);
    layout->addLayout(footer);
    layout->addStretch();
    setCentralWidget(central);
    connect(controller_, &LauncherController::changed, this, &MainWindow::render);
    // Explicit tab order covering every focusable control. Qt would otherwise
    // derive the chain from creation order, which puts the information rows
    // (created first, each with a focusable value label) before the primary
    // action. Actions precede the rows so that when an operation disables
    // Launch mid-flight, the automatic focus hand-off lands on Cancel, not on
    // a static label; the rows are inspection targets and close the cycle.
    const auto tabChain = QList<QWidget *>{launch_, backend_, repair_, refresh_, cancel_, restart_,
        status_, details_, help,
        proxy_->valueLabel(), launchMethod_->valueLabel(), app_->valueLabel(),
        entry_->valueLabel(), process_->valueLabel(), coverage_->valueLabel()};
    for (int i = 0; i + 1 < tabChain.size(); ++i) setTabOrder(tabChain[i], tabChain[i + 1]);
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
    const auto activateFocused = [this] {
        auto *focused = QApplication::focusWidget();
        if (auto *button = qobject_cast<QAbstractButton *>(focused)) button->click();
        else if (auto *label = qobject_cast<ElidedLabel *>(focused)) label->toggleExpanded();
        // Launch only when Enter arrives with no meaningful focus: firing a
        // launch while the user is reading an expanded value would be a
        // surprise action, the exact thing the state boundary forbids.
        else if (!focused || focused == this) controller_->launch();
    };
    // Each binding records its own key and action into the About table, so the
    // documented list and the live QShortcut objects cannot drift apart.
    const auto shortcut = [this](const QKeySequence &keys, const QString &action, auto callback) {
        auto *binding = new QShortcut(keys, this);
        binding->setContext(Qt::WindowShortcut);
        connect(binding, &QShortcut::activated, this, callback);
        shortcutRows_.append({binding->key().toString(QKeySequence::NativeText), action});
        return binding;
    };
    const auto activateAction = QStringLiteral("Activate the focused control (Launch when nothing is focused)");
    shortcut(QKeySequence(Qt::Key_Return), activateAction, activateFocused);
    shortcut(QKeySequence(Qt::Key_Enter), activateAction, activateFocused);
    const auto proxyAction = QStringLiteral("Open proxy settings");
    shortcut(QKeySequence("Ctrl+,"), proxyAction, [this] { proxyDialog(); });
    shortcut(QKeySequence("C"), proxyAction, [this] { proxyDialog(); });
    shortcut(QKeySequence("B"), QStringLiteral("Open the Codex backend proxy consent"), [this] { backendDialog(); });
    shortcut(QKeySequence("D"), QStringLiteral("Open the repair-and-launch confirmation"), [this] { repairDialog(); });
    const auto refreshAction = QStringLiteral("Refresh local state");
    shortcut(QKeySequence("R"), refreshAction, [this] { controller_->refresh(); });
    shortcut(QKeySequence(Qt::Key_F5), refreshAction, [this] { controller_->refresh(); });
    const auto aboutAction = QStringLiteral("About Codex Proxy Guard");
    shortcut(QKeySequence(Qt::Key_F1), aboutAction, [this] { aboutDialog(); });
    shortcut(QKeySequence("?"), aboutAction, [this] { aboutDialog(); });
    // Esc is not a binding of this window; every dialog here closes on it
    // through QDialog's default handling, which is worth stating in the table.
    shortcutRows_.append({QStringLiteral("Esc"), QStringLiteral("Close the open dialog")});
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
    const auto methodLabels = QMap<QString, QString>{{"appmodel_activation", "Windows application activation"}, {"native_process", "Process environment"}};
    const auto method = s.value("launch").toObject().value("method").toString();
    launchMethod_->setValue(knownText(method, methodLabels));
    launchMethod_->setTooltip(explainUnknown(method, methodLabels));
    app_->setValue(desktop.value("display_name").toString("Not discovered") + "  " + desktop.value("package_version").toString());
    entry_->setValue(desktop.value("manifest_executable").toString("Unknown"));
    const auto processState = process.value("state").toString();
    const auto processLabels = QMap<QString, QString>{{"running", "Running"}, {"stopped", "Stopped"}};
    process_->setValue(knownText(processState, processLabels));
    process_->setTone(processState == "running" ? t.success : t.secondary);
    process_->setTooltip(explainUnknown(processState, processLabels));
    const auto coverageLabels = QMap<QString, QString>{{"not_authorized", "Setup required"}, {"not_applicable", "Process environment"}, {"pending", "Pending next launch"}, {"current", "Configuration current"}, {"stale", "Changed · launch to update"}, {"conflict", "Configuration conflict"}, {"invalid", "Invalid configuration"}, {"unavailable", "Unavailable"}, {"unsupported", "Cannot auto-edit · fix .env manually"}};
    coverage_->setValue(knownText(state, coverageLabels));
    const bool coverageError = state == "conflict" || state == "invalid" || state == "unavailable";
    coverage_->setTone(state == "current" ? t.success : coverageError ? t.danger : state == "unknown" || state.isEmpty() ? t.secondary : t.warning);
    coverage_->setTooltip(state == "unsupported"
        ? "The Codex Home .env uses multi-line or unterminated quoting. Guard refuses to edit it automatically; no network state is implied. Fix the file manually, then retry."
        : explainUnknown(state, coverageLabels));
    launch_->setEnabled(controller_->allows("can_launch"));
    // A disabled control must state its own reason; the alternative is the user
    // hunting through Details for a condition the engine already reported. The
    // status line below renders the same reason with error-code priority.
    const auto blocked = launchBlockReason(s, controller_->busy());
    launch_->setToolTip(controller_->allows("can_launch") ? QString() : blocked);
    edit_->setEnabled(controller_->allows("can_edit_proxy"));
    backend_->setEnabled(controller_->allows("can_authorize_backend_proxy"));
    repair_->setEnabled(controller_->allows("can_repair"));
    refresh_->setEnabled(controller_->connected() && !controller_->busy() && !controller_->closing());
    // Always visible, disabled when not applicable: a row that gains and loses
    // buttons moved every control beside it on each state change.
    cancel_->setEnabled(controller_->operating() && !controller_->closing());
    cancel_->setToolTip(controller_->operating()
        ? "Cancel the running operation"
        : "No operation is running");
    restart_->setEnabled(!controller_->connected() && !controller_->busy() && !controller_->closing());
    restart_->setToolTip(controller_->connected()
        ? "The engine is connected; restart is only offered when it is not"
        : "Reconnect to the bundled engine");
    progress_->setVisible(controller_->busy());
    launch_->setText(controller_->busy() ? "Working…" : processState == "running" ? "ChatGPT Desktop is running" : "Launch ChatGPT Desktop");
    const auto badge = !controller_->connected() ? "OFFLINE" : controller_->busy() ? "WORKING" : processState == "running" ? "RUNNING" : controller_->allows("can_launch") ? "READY" : "BLOCKED";
    badge_->setText(badge);
    badge_->setStyleSheet("color:" + (controller_->allows("can_launch") || processState == "running" ? t.success : t.secondary).name());
    const bool error = !controller_->errorCode().isEmpty();
    // Priority: a real engine error explains more than a blocked launch, and a
    // blocked launch explains more than a generic status message.
    // `error` is declared after `muted` in the stylesheet, so it wins when both
    // match.
    setDynamicProperty(status_, "error", error);
    if (error) {
        // The human message leads; the machine code is what Details and the
        // copied diagnostic carry, so the status line stays readable.
        status_->setValue(controller_->errorMessage());
        status_->setTooltip(controller_->errorCode());
    } else if (!blocked.isEmpty()) {
        status_->setValue(blocked);
        status_->setTooltip(QStringLiteral("Launch is unavailable until this is resolved: ") + blocked);
    } else {
        status_->setValue(controller_->message());
        status_->setTooltip(controller_->message());
    }
    // Always visible, disabled when there is nothing to show: hiding a footer
    // button shifted its neighbours on every state change.
    details_->setEnabled(error || !controller_->receipt().isEmpty());
    details_->setToolTip(details_->isEnabled()
        ? QString()
        : QStringLiteral("Details become available after a launch or when the engine reports an error"));
}
void MainWindow::proxyDialog() {
    if (!controller_->allows("can_edit_proxy")) return;
    if (!proxyDialog_) proxyDialog_ = new ProxySettingsDialog(controller_, this);
    proxyDialog_->reset();
    proxyDialog_->open();
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
QString MainWindow::receiptSummary(const QJsonObject &receipt) {
    const bool activation = receipt.value("launch_method").toString() == "appmodel_activation";
    const QString instance = receipt.value("instance").toString();
    const QString instanceText = !activation ? QString("Not applicable")
        : instance == "reused" ? "Reused an existing instance; this launch's proxy settings may not have been re-applied"
        : instance == "unknown" ? "Unknown; whether a new instance was created is unconfirmed"
        : "Created (new instance observed)";
    return QString("Launch: %1\nInstance: %2\nProxy delivery (how configuration was submitted): %3\nBackend configuration: %4\nDaemon: %5\nPID: %6\n\nThese are engine observations, not network verification.")
        .arg(receipt.value("launch_method").toString(), instanceText, receipt.value("proxy_delivery").toString(),
             receipt.value("backend_proxy_config").toString(), receipt.value("daemon_preparation").toString(),
             QString::number(receipt.value("pid").toInt()));
}
void MainWindow::errorDialog() {
    QDialog dialog(this); dialog.setWindowTitle("Action details"); dialog.setMinimumWidth(430);
    auto *layout = new QVBoxLayout(&dialog); layout->setSpacing(16);
    const auto receipt = controller_->receipt();
    // With an error the code and human message lead; otherwise the full status
    // message comes first — the status line elides it to two lines, so Details
    // is where the ~180-character text is always available whole.
    const auto detailText = controller_->errorCode().isEmpty()
        ? controller_->message() + "\n\n" + receiptSummary(receipt)
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
    // Copies are invisible by nature; the Licenses dialog already proves the
    // pattern of a status line that confirms and then restores itself.
    auto *copyStatus = textLabel(QString(), &dialog);
    copyStatus->setObjectName("copyStatus");
    copyStatus->setProperty("muted", true);
    copyStatus->setVisible(false);
    auto *copyFeedback = new QTimer(&dialog);
    copyFeedback->setSingleShot(true);
    connect(copyFeedback, &QTimer::timeout, &dialog, [copyStatus] { copyStatus->setVisible(false); });
    connect(copy, &QPushButton::clicked, &dialog, [this, copyStatus, copyFeedback] {
        const auto s = controller_->snapshot();
        QApplication::clipboard()->setText(QStringLiteral("Codex Proxy Guard %1\nCommit: %2\nError: %3\nDesktop: %4\nCoverage: %5")
            .arg(CPG_PRODUCT_VERSION, CPG_BUILD_COMMIT, controller_->errorCode(), s.value("desktop").toObject().value("package_version").toString(), s.value("coverage").toObject().value("state").toString()));
        copyStatus->setText("Diagnostic copied to the clipboard.");
        copyStatus->setVisible(true);
        copyFeedback->start(1500);
    });
    layout->addWidget(copyStatus);
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
QString MainWindow::aboutText() const {
    return QStringLiteral("Codex Proxy Guard %1\nQt %2 · protocol 1\n\n%3\n\nProxy preparation is not network verification.\nClosing Guard never terminates Desktop.\n\nApplication: MIT · Qt: LGPLv3\nComplete license texts are embedded in this executable.")
        .arg(CPG_PRODUCT_VERSION, qVersion(), formatShortcuts(shortcutRows_));
}
void MainWindow::aboutDialog() {
    QDialog dialog(this);
    dialog.setWindowTitle("About Codex Proxy Guard");
    dialog.setMinimumWidth(430);
    auto *layout = new QVBoxLayout(&dialog);
    layout->setSpacing(16);
    auto *about = textLabel(aboutText(), &dialog);
    about->setTextInteractionFlags(Qt::TextSelectableByMouse | Qt::TextSelectableByKeyboard);
    layout->addWidget(about);
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Close, &dialog);
    auto *licenses = buttons->addButton("Licenses / Third-party notices", QDialogButtonBox::ActionRole);
    licenses->setAccessibleName("Open the embedded licenses and third-party notices");
    // The licenses window is modeless; the About dialog is modal, so it steps
    // aside instead of blocking the reader it just opened.
    connect(licenses, &QPushButton::clicked, &dialog, [this, &dialog] { dialog.close(); licensesDialog(); });
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    layout->addWidget(buttons);
    dialog.exec();
}
void MainWindow::licensesDialog() {
    if (!licensesDialog_) licensesDialog_ = new LicensesDialog(this);
    licensesDialog_->present();
}
void MainWindow::closeEvent(QCloseEvent *event) {
    if (closeReady_) { event->accept(); return; }
    event->ignore();
    if (controller_->closing()) return;
    if (controller_->busy() && !confirm(this, "Close Guard", "An operation is still running. Cancel it and exit? Guard will wait for bounded cleanup. Desktop will remain running if activation has already happened.", "Cancel operation & exit")) return;
    controller_->close();
}
bool MainWindow::eventFilter(QObject *watched, QEvent *event) {
    // Keeps the progress overlay glued to the bottom edge of the information
    // card as the card resizes; it has no layout, so this is its only geometry
    // source after construction.
    if (watched == rows_ && event->type() == QEvent::Resize)
        progress_->setGeometry(1, rows_->height() - 4, rows_->width() - 2, 3);
    return QMainWindow::eventFilter(watched, event);
}
void MainWindow::showEvent(QShowEvent *event) {
    QMainWindow::showEvent(event);
    // The initial focus belongs on the primary action. It cannot be set in the
    // constructor: the constructor-time render() runs before the engine is
    // connected and disables Launch, and Qt would immediately hand the focus
    // to the next control in the chain. By the first show the state is real;
    // when Launch is unavailable the default chain walk finds the first
    // actionable control instead.
    if (!shownOnce_) {
        shownOnce_ = true;
        if (launch_->isEnabled()) launch_->setFocus(Qt::OtherFocusReason);
    }
}
}
