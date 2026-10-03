#pragma once

#include <QApplication>
#include <QString>

namespace guard {

// Run the GUI role. The caller classified the process into this role and
// constructed exactly one QApplication. configPath is empty for the default
// Guard configuration, otherwise an absolute path forwarded to the bridge
// child so GUI and engine observe the same isolated configuration.
int runGui(QApplication &app, const QString &configPath, bool smokeTest);

}
