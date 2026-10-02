#include "engine/engine_bridge.h"
#include "controller/launcher_controller.h"
#include "ui/main_window.h"
#include "ui/theme.h"
#include <QApplication>
#include <QFile>
#include <QJsonDocument>
#include <QStyleHints>
#include <QTimer>
#include <cstdio>

int main(int argc, char *argv[]) {
    QApplication app(argc, argv);
    QCoreApplication::setApplicationName("Codex Proxy Guard");
    QCoreApplication::setApplicationVersion(CPG_PRODUCT_VERSION);
    if (app.arguments().contains("--build-info")) {
        const QJsonObject info{{"product_version", CPG_PRODUCT_VERSION}, {"git_commit", CPG_BUILD_COMMIT},
            {"git_dirty", QStringLiteral(CPG_BUILD_DIRTY) != "false"}, {"protocol_version", 1}, {"qt_version", qVersion()}};
        const auto json = QJsonDocument(info).toJson(QJsonDocument::Compact);
        std::fwrite(json.constData(), 1, static_cast<size_t>(json.size()), stdout);
        std::fflush(stdout);
        return 0;
    }
    guard::applyTheme(app);
    QObject::connect(QGuiApplication::styleHints(), &QStyleHints::colorSchemeChanged, &app, [&app] { guard::applyTheme(app); });
    guard::EngineBridge engine;
    guard::LauncherController controller(&engine);
    guard::MainWindow window(&controller);
    window.show();
    if (app.arguments().contains("--smoke-test")) {
        QTimer::singleShot(100, &window, &QWidget::close);
    } else {
        QTimer::singleShot(0, &controller, &guard::LauncherController::start);
    }
    return app.exec();
}
