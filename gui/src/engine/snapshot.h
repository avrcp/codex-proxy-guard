#pragma once

#include <QJsonObject>
#include <QString>

namespace guard {

enum class DesktopState { Unknown, Found, Missing };
enum class ProcessState { Unknown, Stopped, Running };
enum class Coverage { Unknown, NotApplicable, NotAuthorized, Pending, Current, Stale, Conflict, Invalid, Unavailable };

struct Snapshot {
    QString configReadiness;
    QString proxyUrl;
    QString proxyHost;
    int proxyPort = 0;
    QString launchMethod;
    DesktopState desktopState = DesktopState::Unknown;
    QString displayName;
    QString packageVersion;
    QString architecture;
    QString applicationId;
    QString manifestExecutable;
    QString runtimeKind;
    ProcessState processState = ProcessState::Unknown;
    quint64 pid = 0;
    Coverage coverage = Coverage::Unknown;
    bool backendEnabled = false;
    QString authorizedHome;
    QString candidateHome;
    bool canLaunch = false;
    bool canEditProxy = false;
    bool canAuthorizeBackendProxy = false;
    bool canRevokeBackendProxy = false;
    bool canRepair = false;
    bool canCancel = false;
    bool canRefresh = false;
    bool busy = false;
    QString elevation;
    QString backendConfirmationToken;
    QString backendConfirmationHome;
    bool backendConfirmationEnabled = false;
    QString repairConfirmationToken;
    QString errorCode;
    QString errorMessage;

    static Snapshot fromJson(const QJsonObject &object);
};

} // namespace guard
