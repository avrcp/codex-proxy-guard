#pragma once

#include "launch.h"

#include <chrono>
#include <future>
#include <optional>
#include <string>
#include <vector>

namespace cpg {

// Candidate validation/authorization is synchronous; exactly one committed
// effect executes off-thread. Results are applied only by the session owner.
// Interim migration component: the TUI session (session.h) extracts the
// transport-free half; both are deleted together with the bridge role.
class BridgeSession {
public:
    explicit BridgeSession(std::filesystem::path path,
        std::function<Desktop(const Config &, const Cancellation &)> discovery = discoverDesktop,
        std::function<std::string()> queryElevation = elevation);
    ~BridgeSession();
    std::vector<Json> request(const std::string &frame);
    std::vector<Json> poll();
    void cancel();
    bool closed() const { return closed_; }
    bool busy() const { return pending_; }
private:
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
    std::filesystem::path path_;
    std::function<Desktop(const Config &, const Cancellation &)> discovery_;
    std::function<std::string()> queryElevation_;
    Result state_;
    bool hello_ = false, closed_ = false, pending_ = false, launching_ = false;
    std::int64_t lastId_ = 0, pendingId_ = 0, operation_ = 0;
    std::string consentToken_, repairToken_;
    std::filesystem::path confirmedHome_;
    Config confirmedConfig_;
    Cancellation cancellation_;
    std::future<Result> task_;
    std::chrono::steady_clock::time_point timer_;
    void invalidate();
    void confirmations();
    Json snapshot() const;
    void dispatch(std::int64_t id, bool launch, std::function<Result()> effect);
    Result refresh(const Cancellation &cancel) const;
};
int runBridge(const std::filesystem::path &configPath);

} // namespace cpg
