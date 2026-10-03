#pragma once
#include "controller/launcher_controller.h"
#include <QMainWindow>
#include <QPair>
#include <QVector>
class QLabel;
class QPushButton;
class QProgressBar;
class QFrame;
namespace guard {
class InfoRow;
class LicensesDialog;
class ProxySettingsDialog;
class ElidedLabel;
class MainWindow final : public QMainWindow {
    Q_OBJECT
public:
    explicit MainWindow(LauncherController *controller, QWidget *parent = nullptr);
    // Human-readable receipt observations; distinguishes activation success
    // from instance reuse and never claims network verification.
    static QString receiptSummary(const QJsonObject &receipt);
    // The About body, including the shortcut table derived from the registered
    // QShortcut bindings; public so a test can pin table and bindings together.
    QString aboutText() const;
protected:
    void closeEvent(QCloseEvent *event) override;
    void showEvent(QShowEvent *event) override;
    bool eventFilter(QObject *watched, QEvent *event) override;
private:
    void render();
    void proxyDialog();
    void licensesDialog();
    void backendDialog();
    void repairDialog();
    void errorDialog();
    void aboutDialog();
    LauncherController *controller_;
    InfoRow *proxy_, *launchMethod_, *app_, *entry_, *process_, *coverage_;
    QLabel *badge_;
    // Elided so a ~180-character engine message cannot push the window past
    // the 480x400 minimum DESIGN.md promises.
    ElidedLabel *status_;
    QPushButton *launch_, *edit_, *backend_, *repair_, *refresh_, *cancel_, *restart_, *details_;
    QFrame *rows_ = nullptr;   // the information card; hosts the progress overlay
    QProgressBar *progress_;
    ProxySettingsDialog *proxyDialog_ = nullptr;
    LicensesDialog *licensesDialog_ = nullptr;
    // Key / action pairs captured from the registered QShortcut bindings; the
    // only place the window's shortcut table is written down.
    QVector<QPair<QString, QString>> shortcutRows_;
    bool closeReady_ = false;
    bool shownOnce_ = false;
};
}
