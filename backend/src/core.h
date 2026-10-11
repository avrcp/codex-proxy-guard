#pragma once

#include "cpg/cancellation.h"
#include "cpg/error.h"
#include "cpg/json.h"
#include "cpg/text.h"

#include <filesystem>
#include <memory>
#include <string>
#include <vector>

namespace cpg {

struct Config {
    int version = 2;
    std::string scheme = "http";
    std::string host = "127.0.0.1";
    int port = 10808;
    std::vector<std::string> noProxy{"localhost", "127.0.0.1", "::1"};

    std::filesystem::path executableOverride;
    std::filesystem::path cliOverride;

    bool refuseIfRunning = true;
    bool manageBackend = false;
    std::filesystem::path home;

    std::string alternateScreen = "auto";
    bool operator==(const Config &) const = default;
    void validate() const;
    std::string proxyUrl() const;
    std::string noProxyValue() const;
    std::string toml() const;
    static Config parse(const std::string &bytes);
    static Config load(const std::filesystem::path &path);
    static Config loadOrCreate(const std::filesystem::path &path);
    static std::filesystem::path defaultPath();
    static std::filesystem::path defaultHome();
};

// Opaque held Windows handle; a launch lease excludes all config mutations.
class ConfigLease {
public:
    ConfigLease(const std::filesystem::path &path, const Config &expected);
    ~ConfigLease();
    ConfigLease(const ConfigLease &) = delete;
    ConfigLease &operator=(const ConfigLease &) = delete;
private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

void initializeConfig(const std::filesystem::path &path, const Config &config, bool force);
Config updateProxy(const std::filesystem::path &path, const Config &expected, bool invalid,
                   const std::string &host, int port);
Config updateConsent(const std::filesystem::path &path, const Config &expected, bool enable,
                     const std::filesystem::path &confirmedHome);
std::filesystem::path consentHome(const Config &config);
std::string inspectProxyEnv(const Config &config); // unknown/not_authorized/pending/current/stale/conflict/invalid/unavailable/unsupported
void prepareProxyEnv(const Config &config);
void revokeProxyEnv(const std::filesystem::path &home);

} // namespace cpg
