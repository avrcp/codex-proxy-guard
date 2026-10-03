#include "gui_entry.h"
#include "engine/engine_bridge.h"
#include "controller/launcher_controller.h"
#include "ui/main_window.h"
#include "ui/theme.h"
#include <QStyleHints>
#include <QTimer>
#include <QWidget>

namespace guard {

int runGui(QApplication &app, const QString &configPath, bool smokeTest) {
    QCoreApplication::setApplicationName("Codex Proxy Guard");
    QCoreApplication::setApplicationVersion(CPG_PRODUCT_VERSION);
    guard::applyTheme(app);
    QObject::connect(QGuiApplication::styleHints(), &QStyleHints::colorSchemeChanged, &app, [&app] { guard::applyTheme(app); });
    guard::EngineBridge engine(configPath);
    guard::LauncherController controller(&engine);
    guard::MainWindow window(&controller);
    window.show();
    if (smokeTest) {
        QTimer::singleShot(100, &window, &QWidget::close);
    } else {
        QTimer::singleShot(0, &controller, &guard::LauncherController::start);
    }
    return app.exec();
}

}
