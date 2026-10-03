#pragma once

#include <QCoreApplication>

namespace cpg {

// Execute the headless CLI/console/bridge/worker roles. The caller has already
// classified the process into this role and constructed exactly one
// QCoreApplication; runCli owns argument parsing and all business entry points.
int runCli(QCoreApplication &app);

}
