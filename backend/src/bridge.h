#pragma once

#include "launch.h"
#include "session.h"

#include <string>
#include <vector>

namespace cpg {

// Interim NDJSON protocol adapter over the shared EngineSession. The TUI uses
// the session directly; this file exists only so the legacy Qt GUI can keep
// driving the engine during migration and is deleted with the bridge role.
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
    bool busy() const { return session_->busy(); }
private:
    std::int64_t lastId_ = 0, pendingId_ = 0;
    bool hello_ = false, closed_ = false;
    std::unique_ptr<EngineSession> session_;
};
int runBridge(const std::filesystem::path &configPath);

} // namespace cpg
