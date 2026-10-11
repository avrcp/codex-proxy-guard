#pragma once

#include "launch.h"

#include <chrono>
#include <future>
#include <optional>
#include <string>
#include <vector>

namespace cpg {

// Transport-free engine session: the single authoritative state machine for
// every interactive frontend (TUI, plain console and — until it is removed —
// the bridge protocol adapter). It owns candidate validation, single-use
// confirmation tokens, the one-foreground-effect gate and TaskResult
// application. The JSON snapshot it produces is the frontend view model:
// renderers must display it, never recompute business rules from raw engine
// types.
class EngineSession {
public:
    struct Result {
        Config config;
        bool ready = false;
        bool snapshot = false;
        std::optional<Desktop> desktop;
        ProcessState process;
        std::string coverage = "unknown";
        Json value = Json(nullptr);
        std::optional<Error> error;
    };
    enum class Event { None, SnapshotArrived, OperationFinished, MutationFailed };

    explicit EngineSession(std::filesystem::path path,
        std::function<Desktop(const Config &, const Cancellation &)> discovery = discoverDesktop,
        std::function<std::string()> queryElevation = elevation);
    ~EngineSession();
    EngineSession(const EngineSession &) = delete;
    EngineSession &operator=(const EngineSession &) = delete;

    // Asynchronous refresh of config/discovery/process/coverage.
    void requestRefresh();
    // Typed mutations mirroring the bridge method semantics exactly.
    void requestSetProxy(const std::string &host, int port);
    void requestSetBackendConsent(bool enabled, const std::string &token);
    void requestLaunch(bool repair, const std::string &token);
    // Cancel the active foreground operation (safe when idle).
    void cancel();
    // Pump completed tasks; returns the event to surface. busy()/snapshot()
    // reflect the state after the call.
    Event poll();
    bool busy() const { return pending_; }
    bool launching() const { return launching_; }
    // The view model (same shape the bridge protocol emitted).
    Json snapshot() const;
    // The error of a finished foreground operation, when poll() reported
    // OperationFinished with ok == false.
    std::optional<Error> operationError() const { return state_.error; }
    // The receipt of a finished foreground operation on success.
    Json operationResult() const { return state_.value; }
    // The error of a failed mutation (poll() reported MutationFailed).
    std::optional<Error> mutationError() const { return mutationError_; }
    // Monotonic id of the most recently submitted foreground launch.
    std::int64_t operationId() const { return operation_; }

private:
    void invalidate();
    void confirmations();
    void dispatch(bool launch, std::function<Result()> effect);
    Result refresh(const Cancellation &cancel) const;

    std::filesystem::path path_;
    std::function<Desktop(const Config &, const Cancellation &)> discovery_;
    std::function<std::string()> queryElevation_;
    Result state_;
    bool pending_ = false, launching_ = false;
    std::int64_t operation_ = 0;
    std::string consentToken_, repairToken_;
    std::filesystem::path confirmedHome_;
    Config confirmedConfig_;
    Cancellation cancellation_;
    std::future<Result> task_;
    std::chrono::steady_clock::time_point dispatchedAt_;
    bool mutationFailure_ = false;
    std::optional<Error> mutationError_;
};

} // namespace cpg
