#pragma once
#include "controller/launcher_controller.h"
#include <QMainWindow>
class QLabel;
class QPushButton;
class QProgressBar;
namespace guard {
class InfoRow;
class LicensesDialog;
class ProxySettingsDialog;
class MainWindow final : public QMainWindow {
    Q_OBJECT
public:
    explicit MainWindow(LauncherController *controller, QWidget *parent = nullptr);
    // Human-readable receipt observations; distinguishes activation success
    // from instance reuse and never claims network verification.
    static QString receiptSummary(const QJsonObject &receipt);
protected:
    void closeEvent(QCloseEvent *event) override;
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
    QLabel *badge_, *status_;
    QPushButton *launch_, *edit_, *backend_, *repair_, *refresh_, *cancel_, *restart_, *details_;
    QProgressBar *progress_;
    ProxySettingsDialog *proxyDialog_ = nullptr;
    LicensesDialog *licensesDialog_ = nullptr;
    bool closeReady_ = false;
};
}
