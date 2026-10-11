#include "gui_entry.h"
#include "cpg/text.h"
#include "engine/engine_bridge.h"
#include "controller/launcher_controller.h"
#include "ui/main_window.h"
#include "ui/theme.h"
#include <QApplication>
#include <QDir>
#include <QStyleHints>
#include <QTimer>
#include <QWidget>
#include <windows.h>
#include <shellapi.h>
#include <cstring>
#include <memory>
#include <vector>

namespace guard {

namespace {

// QApplication requires argv-style UTF-8 pointers that outlive the
// application object; they are owned by this heap holder.
struct ArgumentHolder {
    int argc = 0;
    std::vector<std::unique_ptr<char[]>> utf8;
    std::vector<char *> argv;

    explicit ArgumentHolder(const std::vector<std::wstring> &arguments)
    {
        utf8.reserve(arguments.size() + 1);
        for (const std::wstring &argument : arguments) {
            std::string bytes = cpg::wideToUtf8(argument);
            auto buffer = std::make_unique<char[]>(bytes.size() + 1);
            std::memcpy(buffer.get(), bytes.data(), bytes.size());
            buffer[bytes.size()] = '\0';
            argv.push_back(buffer.get());
            utf8.push_back(std::move(buffer));
        }
        argv.push_back(nullptr);
        argc = static_cast<int>(argv.size() - 1);
    }
};

} // namespace

int runGui(const std::wstring &configPath, bool smokeTest) {
    std::vector<std::wstring> raw;
    int count = 0;
    if (LPWSTR *wide = CommandLineToArgvW(GetCommandLineW(), &count)) {
        raw.reserve(static_cast<std::size_t>(count));
        for (int index = 0; index < count; ++index)
            raw.emplace_back(wide[index]);
        LocalFree(wide);
    }
    ArgumentHolder holder(raw);
    QApplication app(holder.argc, holder.argv.data());
    QCoreApplication::setApplicationName("Codex Proxy Guard");
    QCoreApplication::setApplicationVersion(CPG_PRODUCT_VERSION);
    guard::applyTheme(app);
    QObject::connect(QGuiApplication::styleHints(), &QStyleHints::colorSchemeChanged, &app, [&app] { guard::applyTheme(app); });
    QString qtConfigPath = QString::fromStdWString(configPath);
    if (!qtConfigPath.isEmpty() && QDir::isRelativePath(qtConfigPath))
        qtConfigPath = QDir(QDir::currentPath()).absoluteFilePath(qtConfigPath);
    guard::EngineBridge engine(qtConfigPath);
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

} // namespace guard
