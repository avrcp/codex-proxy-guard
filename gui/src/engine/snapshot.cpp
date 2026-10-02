#include "snapshot.h"
#include "protocol.h"

namespace guard {

Snapshot Snapshot::fromJson(const QJsonObject &object)
{
    Snapshot s;
    s.configReadiness = object.value("config_readiness").toString();
    const auto proxy = object.value("proxy").toObject();
    s.proxyUrl = proxy.value("url").toString();
    s.proxyHost = proxy.value("host").toString();
    const auto port = proxy.value("port");
    quint64 number = 0;
    if (protocolId(port, &number) && number <= 65535) s.proxyPort = static_cast<int>(number);
    s.launchMethod = object.value("launch").toObject().value("method").toString();
    const auto desktop = object.value("desktop").toObject();
    const auto desktopState = desktop.value("state").toString();
    if (desktopState == "found") s.desktopState = DesktopState::Found;
    else if (desktopState == "missing" || desktopState == "not_found") s.desktopState = DesktopState::Missing;
    s.displayName = desktop.value("display_name").toString();
    s.packageVersion = desktop.value("package_version").toString();
    s.architecture = desktop.value("architecture").toString();
    s.applicationId = desktop.value("application_id").toString();
    s.manifestExecutable = desktop.value("manifest_executable").toString();
    s.runtimeKind = desktop.value("runtime_kind").toString();
    const auto process = object.value("process").toObject();
    const auto processState = process.value("state").toString();
    if (processState == "running") s.processState = ProcessState::Running;
    else if (processState == "stopped") s.processState = ProcessState::Stopped;
    if (protocolId(process.value("pid"), &number)) s.pid = number;
    const auto coverage = object.value("coverage").toObject();
    const auto state = coverage.value("state").toString();
    if (state == "not_applicable") s.coverage = Coverage::NotApplicable;
    else if (state == "not_authorized") s.coverage = Coverage::NotAuthorized;
    else if (state == "pending") s.coverage = Coverage::Pending;
    else if (state == "current") s.coverage = Coverage::Current;
    else if (state == "stale") s.coverage = Coverage::Stale;
    else if (state == "conflict") s.coverage = Coverage::Conflict;
    else if (state == "invalid") s.coverage = Coverage::Invalid;
    else if (state == "unavailable") s.coverage = Coverage::Unavailable;
    s.backendEnabled = coverage.value("enabled").toBool(false);
    s.authorizedHome = coverage.value("authorized_home").toString();
    s.candidateHome = coverage.value("candidate_home").toString();
    s.busy = object.value("busy").toBool(false);
    s.elevation = object.value("elevation").toString();
    const auto confirmations = object.value("confirmations").toObject();
    const auto backend = confirmations.value("backend_proxy").toObject();
    if (backend.value("enabled").isBool()) {
        s.backendConfirmationToken = backend.value("token").toString();
        s.backendConfirmationHome = backend.value("home").toString();
        s.backendConfirmationEnabled = backend.value("enabled").toBool();
    }
    s.repairConfirmationToken = confirmations.value("repair").toObject().value("token").toString();
    const auto actions = object.value("actions").toObject();
    s.canEditProxy = actions.value("can_edit_proxy").toBool(false) && !s.busy;
    s.canRefresh = actions.value("can_refresh").toBool(false) && !s.busy;
    s.canCancel = actions.value("can_cancel").toBool(false) && s.busy;
    const bool hasConsent = !s.backendConfirmationToken.isEmpty() && !s.backendConfirmationHome.isEmpty();
    s.canAuthorizeBackendProxy = actions.value("can_authorize_backend_proxy").toBool(false)
        && !s.busy && hasConsent && s.backendConfirmationEnabled;
    s.canRevokeBackendProxy = actions.value("can_revoke_backend_proxy").toBool(false)
        && !s.busy && hasConsent && !s.backendConfirmationEnabled;
    const bool launchKnown = s.launchMethod == "appmodel_activation" || s.launchMethod == "native_process";
    const bool safeToLaunch = s.configReadiness == "ready" && s.desktopState == DesktopState::Found
        && s.processState == ProcessState::Stopped && s.elevation == "not_elevated" && launchKnown && !s.busy;
    s.canLaunch = actions.value("can_launch").toBool(false) && safeToLaunch;
    s.canRepair = actions.value("can_repair").toBool(false) && safeToLaunch && !s.repairConfirmationToken.isEmpty();
    const auto error = object.value("error").toObject();
    s.errorCode = error.value("code").toString();
    s.errorMessage = error.value("message").toString();
    return s;
}

} // namespace guard
