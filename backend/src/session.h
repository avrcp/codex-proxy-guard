#pragma once

#include "launch.h"
#include <chrono>
#include <future>
#include <optional>
#include <string>

namespace cpg {

struct SessionBudgets {
    std::chrono::milliseconds mutation{60000};
    std::chrono::milliseconds launch{840000};
};
struct ConsentProposal {
    std::string token;
    bool enabled = false;
    std::filesystem::path home;
    std::uint64_t revision = 0;
    bool operator==(const ConsentProposal &) const = default;
};
struct RepairProposal {
    std::string token;
    std::uint64_t revision = 0;
    bool operator==(const RepairProposal &) const = default;
};
struct ProxyView { std::string url, host; int port = 0; };
struct DesktopView {
    std::string state = "unknown", product, displayName, packageVersion, architecture;
    std::optional<std::string> applicationId, manifestExecutable, runtimeKind;
};
struct ProcessObservation {
    std::string state = "unknown";
    std::optional<std::uint32_t> pid;
};
struct CoverageView {
    std::string state = "unknown";
    bool enabled = false;
    std::optional<std::filesystem::path> authorizedHome, candidateHome;
};
struct Capabilities {
    bool canLaunch = false, canEditProxy = false, canAuthorizeBackendProxy = false,
        canRevokeBackendProxy = false, canRepair = false, canRefresh = false, canCancel = false;
    std::string blockedReason;
};
struct SessionView {
    bool ready = false, busy = false;
    ProxyView proxy;
    DesktopView desktop;
    ProcessObservation process;
    CoverageView coverage;
    std::string elevation = "unknown", launchMethod = "unknown";
    Capabilities actions;
    std::optional<Error> error;
    std::optional<ConsentProposal> consent;
    std::optional<RepairProposal> repair;
    Json publicJson() const;
};
struct MutationOutcome {
    std::int64_t operationId = 0;
    bool committed = false;
    std::optional<Config> committedConfig;
    std::optional<Error> mutationError, refreshError;
    bool exceededBudget = false;
};
struct LaunchReceipt {
    std::optional<std::uint32_t> pid;
    std::string launchMethod, activationState, instance, proxyDelivery,
        backendProxyConfig, daemonPreparation;
    std::optional<std::string> proxyEndpoint;
    std::optional<bool> targetElevation;
};
struct LaunchOutcome {
    std::int64_t operationId = 0;
    std::optional<LaunchReceipt> receipt;
    std::optional<Error> error;
    bool exceededBudget = false, needsRefresh = true;
    std::string activationState = "not_submitted", daemonPreparation = "skipped",
        backendProxyConfig = "unknown";
};
// Injection is constructor-only; no production environment or command exposes it.
struct SessionServices {
    std::function<ProcessState(const Desktop &)> process = desktopProcessState;
    std::function<std::filesystem::path(const Config &)> proposalHome = consentHome;
    std::function<Json(const Config &, const std::filesystem::path &, LaunchOptions,
        const Cancellation &, LaunchFacts *)> launch;
};

// The authoritative in-process state boundary. Frontends consume view(); JSON is
// an external adapter. Every foreground future is reaped before the gate opens.
class EngineSession {
public:
    struct Result {
        Config config;
        bool ready = false, snapshot = false;
        std::optional<Desktop> desktop;
        ProcessState process;
        std::string coverage = "unknown";
        Json value = Json(nullptr);
        std::optional<Error> error;
        std::optional<MutationOutcome> mutation;
        std::optional<LaunchOutcome> launch;
    };
    enum class Event { None, SnapshotArrived, OperationFinished, MutationFailed, MutationFinished };
    enum class ForegroundState { Idle, Running, CancellingAfterTimeout };

    explicit EngineSession(std::filesystem::path path,
        std::function<Desktop(const Config &, const Cancellation &)> discovery = discoverDesktop,
        std::function<std::string()> queryElevation = elevation,
        SessionBudgets budgets = {}, SessionServices services = {});
    ~EngineSession();
    EngineSession(const EngineSession &) = delete;
    EngineSession &operator=(const EngineSession &) = delete;
    void requestRefresh();
    void requestSetProxy(const std::string &host, int port);
    void requestSetBackendConsent(const ConsentProposal &proposal);
    void requestSetBackendConsent(bool enabled, const std::string &token);
    void requestLaunch(const RepairProposal &proposal);
    void requestLaunch(bool repair, const std::string &token);
    void cancel();
    Event poll();
    bool busy() const { return pending_; }
    bool launching() const { return launching_; }
    ForegroundState foregroundState() const { return foreground_; }
    bool cancellingAfterTimeout() const { return foreground_ == ForegroundState::CancellingAfterTimeout; }
    SessionView view() const;
    Json snapshot() const { return view().publicJson(); }
    std::optional<Error> operationError() const { return launchOutcome_ ? launchOutcome_->error : std::nullopt; }
    Json operationResult() const { return operationResult_; }
    std::optional<Error> mutationError() const { return mutationError_; }
    const std::optional<MutationOutcome> &mutationOutcome() const { return mutationOutcome_; }
    const std::optional<LaunchOutcome> &launchOutcome() const { return launchOutcome_; }
    std::int64_t operationId() const { return operation_; }

private:
    void invalidate();
    void confirmations();
    void dispatch(bool launch, std::function<Result()> effect);
    Result refresh(const Cancellation &cancel) const;
    Result committedRefresh(Config config, const Cancellation &cancel) const;
    std::filesystem::path path_;
    std::function<Desktop(const Config &, const Cancellation &)> discovery_;
    std::function<std::string()> queryElevation_;
    SessionBudgets budgets_;
    SessionServices services_;
    Result state_;
    bool pending_ = false, launching_ = false;
    ForegroundState foreground_ = ForegroundState::Idle;
    std::int64_t operation_ = 0;
    std::uint64_t revision_ = 0;
    std::optional<ConsentProposal> consent_;
    std::optional<RepairProposal> repair_;
    Config confirmedConfig_;
    Cancellation cancellation_;
    std::future<Result> task_;
    std::chrono::steady_clock::time_point dispatchedAt_;
    std::optional<Error> mutationError_;
    std::optional<MutationOutcome> mutationOutcome_;
    std::optional<LaunchOutcome> launchOutcome_;
    Json operationResult_ = nullptr;
};
} // namespace cpg
