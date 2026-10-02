#pragma once
#include "launch.h"

namespace cpg::detail {
// Fixture seams use the same orchestration as production, without launching real
// Desktop processes, touching a user's Home, or interrupting a shared daemon.
QString daemonStopStatus(const QByteArray &output);
QJsonObject activationReceipt(const QJsonObject &worker);
CodexCli resolveCliFrom(const QString &explicitHome, const QString &userHome,
                       const QString &path, const QString &overridePath, const QString &cwd);
struct LaunchServices {
    std::function<void()> requireElevation;
    std::function<Desktop()> discover;
    std::function<ProcessState(const Desktop &)> process;
    std::function<void()> prepare;
    std::function<CodexCli()> resolve;
    std::function<QString(const CodexCli &)> stop;
    std::function<QJsonObject(const Desktop &, const QString &)> activate;
    std::function<QJsonObject(const Desktop &, const QProcessEnvironment &)> native;
    std::function<void()> markSubmitted;
};
// The caller owns both launch/configuration leases for the whole call.
QJsonObject launchWith(const Config &config, LaunchOptions options,
                       const Cancellation &cancel, const LaunchServices &services);
}
