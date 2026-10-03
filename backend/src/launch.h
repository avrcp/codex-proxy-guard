#pragma once
#include "platform.h"
#include <QProcessEnvironment>
namespace cpg {
struct LaunchOptions { bool repair = false; bool activationOnly = false; };
struct ProxyPlan { QString endpoint; QString arguments; };
struct CodexCli { QString executable; QString home; };
ProxyPlan proxyPlan(const Config &config);
QProcessEnvironment proxyEnvironment(const Config &config);
CodexCli resolveCodexCli(const Config &config);
QString stopCodexDaemon(const CodexCli &cli, const Cancellation &cancel, int budgetMs = 720000);
// The single authoritative pipeline for bridge, CLI and console frontend.
QJsonObject launchPipeline(const Config &config, const QString &configPath,
                           LaunchOptions options, const Cancellation &cancel);
}
