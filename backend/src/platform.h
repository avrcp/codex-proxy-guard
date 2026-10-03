#pragma once
#include "core.h"
#include <QJsonObject>
#include <QProcessEnvironment>
#include <functional>

namespace cpg {
struct Desktop {
    QString product = "chat_gpt";
    QString displayName = "ChatGPT Desktop";
    QString packageName, packageVersion, architecture, discoverySource;
    bool registered = false;
    QString executable, installLocation, packageFullName, packageFamilyName;
    QString applicationId, aumid, manifestExecutable, runtimeKind;
    QJsonObject publicJson() const;
};
struct ProcessState {
    QString state = "unknown";
    quint32 pid = 0;
};
struct HelperResult {
    QByteArray out, err;
    int exitCode = -1;
    bool submitted = false;
};
// Only owns/kills its direct short-lived helper, never a process tree.
HelperResult runHelper(const QString &program, const QStringList &arguments,
                       const QByteArray &input, const Cancellation &cancel,
                       int timeoutMs, qsizetype outputLimit,
                       const QProcessEnvironment &environment = QProcessEnvironment::systemEnvironment(),
                       bool activationSubmission = false);
QString elevation(); // not_elevated/elevated/unknown; never treat query failure as safe
void requireNonElevated();
Desktop discoverDesktop(const Config &config, const Cancellation &cancel);
// Exposed for bounded fixture tests; validates schema and registered path containment.
Desktop parseDiscovery(const QByteArray &json, const QString &overrideExecutable = {});
ProcessState desktopProcessState(const Desktop &desktop);
QJsonObject activateDesktop(const Desktop &desktop, const QString &arguments, const Cancellation &cancel);
int activationWorker();
QString normalizedWindowsPath(const QString &path);
}
