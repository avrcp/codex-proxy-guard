#define DOCTEST_CONFIG_IMPLEMENT
#ifdef _MSC_VER
#pragma warning(push, 0)
#endif
#include "doctest.h"
#ifdef _MSC_VER
#pragma warning(pop)
#endif
#include "session.h"
#include "cpg/win_handle.h"
#include "test_support.h"
#include <atomic>
#include <windows.h>
using namespace cpg;
using namespace cpgtest;

namespace {
void settle(EngineSession &session)
{
    for (int count = 0; session.busy() && count < 5000; ++count) {
        session.poll();
        Sleep(1);
    }
    REQUIRE_FALSE(session.busy());
}
}

TEST_CASE("idle cancellation preserves the displayed confirmation proposal")
{
    TemporaryDirectory temporary;
    Config config;
    config.home = temporary.path;
    const auto path = temporary.file(L"guard.toml");
    initializeConfig(path, config, false);
    EngineSession session(path, [](const Config &, const Cancellation &) -> Desktop {
        throw Error("CODEX_NOT_INSTALLED", "Fixture not installed.");
    }, [] { return std::string("not_elevated"); });
    session.requestRefresh();
    settle(session);
    const auto before = session.snapshot().at("confirmations").at("backend_proxy");
    REQUIRE_FALSE(before.is_null());
    session.cancel();
    CHECK(session.snapshot().at("confirmations").at("backend_proxy") == before);
}

TEST_CASE("committed proxy remains applied when post-commit discovery is cancelled")
{
    TemporaryDirectory temporary;
    Config config;
    config.home = temporary.path;
    const auto path = temporary.file(L"guard.toml");
    initializeConfig(path, config, false);
    std::atomic<bool> cancelRefresh{false};
    EngineSession session(path, [&](const Config &, const Cancellation &cancel) -> Desktop {
        if (cancelRefresh) { cancel.cancel(); cancel.check(); }
        throw Error("CODEX_NOT_INSTALLED", "Fixture not installed.");
    }, [] { return std::string("not_elevated"); });
    session.requestRefresh();
    settle(session);
    cancelRefresh = true;
    session.requestSetProxy("localhost", 7890);
    settle(session);
    CHECK(Config::load(path).port == 7890);
    CHECK(session.snapshot().at("proxy").at("port") == 7890);
    REQUIRE(session.mutationOutcome());
    CHECK(session.mutationOutcome()->committed);
    CHECK_FALSE(session.mutationOutcome()->mutationError);
    REQUIRE(session.mutationOutcome()->refreshError);
}

TEST_CASE("typed proposals preserve exact scope reject stale snapshots and support absent candidates")
{
    TemporaryDirectory temporary;
    Config config; config.home = temporary.path;
    const auto path = temporary.file(L"guard.toml");
    initializeConfig(path, config, false);
    SessionServices services;
    services.process = [](const Desktop &) { return ProcessState{"stopped", 0}; };
    EngineSession session(path, [](const Config &, const Cancellation &) { return Desktop{}; },
        [] { return std::string("not_elevated"); }, {}, services);
    session.requestRefresh(); settle(session);
    const auto initial = session.view();
    REQUIRE(initial.consent);
    CHECK(initial.consent->home == temporary.path);
    CHECK_FALSE(initial.coverage.authorizedHome);
    CHECK_FALSE(initial.desktop.applicationId);
    CHECK_FALSE(initial.desktop.runtimeKind);
    const auto displayed = *initial.consent;
    session.requestRefresh(); settle(session);
    CHECK_THROWS_AS(session.requestSetBackendConsent(displayed), Error);
    CHECK_FALSE(Config::load(path).manageBackend);
    session.requestRefresh(); settle(session);
    const auto proposal = *session.view().consent;
    config.port = 7891;
    initializeConfig(path, config, true);
    CHECK(errorCode([&] { session.requestSetBackendConsent(proposal); }) == "CONFIG_CHANGED");
    CHECK_FALSE(std::filesystem::exists(temporary.path / L".env"));
    services.proposalHome = [](const Config &) { return std::filesystem::path{}; };
    EngineSession noCandidate(path, [](const Config &, const Cancellation &) { return Desktop{}; },
        [] { return std::string("not_elevated"); }, {}, services);
    noCandidate.requestRefresh(); settle(noCandidate);
    CHECK_FALSE(noCandidate.view().consent);
    CHECK_FALSE(noCandidate.view().actions.canAuthorizeBackendProxy);
}

TEST_CASE("a proposal is consumed before configuration read failure and cannot be replayed")
{
    for (const bool repair : {false, true}) {
        CAPTURE(repair);
        TemporaryDirectory temporary;
        Config config; config.home = temporary.path;
        const auto path = temporary.file(L"guard.toml");
        initializeConfig(path, config, false);
        const std::string original = readBytes(path);
        int launches = 0;
        SessionServices services;
        services.process = [](const Desktop &) { return ProcessState{"stopped", 0}; };
        services.launch = [&](const Config &, const std::filesystem::path &, LaunchOptions,
            const Cancellation &, LaunchFacts *) { ++launches; return Json::object(); };
        EngineSession session(path, [](const Config &, const Cancellation &) { return Desktop{}; },
            [] { return std::string("not_elevated"); }, {}, services);
        session.requestRefresh(); settle(session);
        REQUIRE(session.view().consent);
        REQUIRE(session.view().repair);
        const ConsentProposal consent = *session.view().consent;
        const RepairProposal repairProposal = *session.view().repair;
        const auto submit = [&] {
            if (repair) session.requestLaunch(repairProposal);
            else session.requestSetBackendConsent(consent);
        };
        {
            const UniqueHandle lock(CreateFileW(path.c_str(), GENERIC_READ, 0, nullptr,
                OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
            REQUIRE(lock.valid());
            const std::string firstFailure = errorCode(submit);
            CHECK_FALSE(firstFailure.empty());
            CHECK(firstFailure != "CONFIRMATION_REQUIRED");
            // A replay must fail before attempting another file read, even
            // while the original exclusive file handle is still held.
            CHECK(errorCode(submit) == "CONFIRMATION_REQUIRED");
            CHECK_FALSE(session.view().consent);
            CHECK_FALSE(session.view().repair);
        }
        CHECK(errorCode(submit) == "CONFIRMATION_REQUIRED");
        CHECK_FALSE(session.busy());
        CHECK(launches == 0); // no activation or daemon-stop pipeline dispatched
        CHECK(readBytes(path) == original);
        CHECK_FALSE(std::filesystem::exists(temporary.path / L".env"));
    }
}

TEST_CASE("a guessed proposal burns the displayed proposal before any configuration read")
{
    for (const bool repair : {false, true}) {
        CAPTURE(repair);
        TemporaryDirectory temporary;
        Config config; config.home = temporary.path;
        const auto path = temporary.file(L"guard.toml");
        initializeConfig(path, config, false);
        SessionServices services;
        services.process = [](const Desktop &) { return ProcessState{"stopped", 0}; };
        EngineSession session(path, [](const Config &, const Cancellation &) { return Desktop{}; },
            [] { return std::string("not_elevated"); }, {}, services);
        session.requestRefresh(); settle(session);
        REQUIRE(session.view().consent);
        REQUIRE(session.view().repair);
        const ConsentProposal consent = *session.view().consent;
        const RepairProposal repairProposal = *session.view().repair;
        auto guessedConsent = consent; guessedConsent.token = "guessed";
        auto guessedRepair = repairProposal; guessedRepair.token = "guessed";
        const UniqueHandle lock(CreateFileW(path.c_str(), GENERIC_READ, 0, nullptr,
            OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
        REQUIRE(lock.valid());
        CHECK(errorCode([&] {
            if (repair) session.requestLaunch(guessedRepair);
            else session.requestSetBackendConsent(guessedConsent);
        }) == "CONFIRMATION_REQUIRED");
        CHECK(errorCode([&] {
            if (repair) session.requestLaunch(repairProposal);
            else session.requestSetBackendConsent(consent);
        }) == "CONFIRMATION_REQUIRED");
        CHECK_FALSE(session.busy());
    }
}

TEST_CASE("timed out mutation retains committed config and disallows overlapping operations")
{
    TemporaryDirectory temporary;
    Config config; config.home = temporary.path;
    const auto path = temporary.file(L"guard.toml");
    initializeConfig(path, config, false);
    std::atomic<bool> slow{false}, entered{false}, release{false};
    SessionBudgets budgets; budgets.mutation = std::chrono::milliseconds(5);
    EngineSession session(path, [&](const Config &, const Cancellation &) -> Desktop {
        if (slow) {
            entered = true;
            while (!release) Sleep(1);
            throw Error("APPX_DISCOVERY_FAILED", "Fixture read failed after commit.");
        }
        return Desktop{};
    }, [] { return std::string("not_elevated"); }, budgets);
    session.requestRefresh(); settle(session);
    slow = true;
    session.requestSetProxy("localhost", 7890);
    for (int count = 0; !entered && count < 1000; ++count) Sleep(1);
    CHECK(entered);
    Sleep(10); session.poll();
    CHECK(session.cancellingAfterTimeout());
    CHECK(session.busy());
    CHECK_THROWS_AS(session.requestSetProxy("127.0.0.1", 7892), Error);
    release = true; settle(session);
    REQUIRE(session.mutationOutcome());
    CHECK(session.mutationOutcome()->committed);
    CHECK(session.mutationOutcome()->exceededBudget);
    CHECK(session.mutationOutcome()->refreshError->code == "APPX_DISCOVERY_FAILED");
    CHECK(session.view().proxy.port == 7890);
    CHECK(Config::load(path).port == 7890);
}

TEST_CASE("timed out launch keeps a verified receipt and later refresh cannot replace it")
{
    TemporaryDirectory temporary;
    Config config; config.home = temporary.path;
    const auto path = temporary.file(L"guard.toml");
    initializeConfig(path, config, false);
    std::atomic<bool> release{false}, entered{false};
    SessionServices services;
    services.process = [](const Desktop &) { return ProcessState{"stopped", 0}; };
    services.launch = [&](const Config &, const std::filesystem::path &, LaunchOptions,
        const Cancellation &, LaunchFacts *facts) {
        entered = true;
        while (!release) Sleep(1);
        facts->activationState = "returned";
        return Json{{"pid", 42}, {"instance", "created"}, {"activation_state", "returned"},
            {"proxy_delivery", "activation_arguments"}, {"daemon_preparation", "skipped"},
            {"backend_proxy_config", "not_authorized"}};
    };
    SessionBudgets budgets; budgets.launch = std::chrono::milliseconds(5);
    EngineSession session(path, [](const Config &, const Cancellation &) { return Desktop{}; },
        [] { return std::string("not_elevated"); }, budgets, services);
    session.requestRefresh(); settle(session);
    session.requestLaunch(false, "");
    for (int count = 0; !entered && count < 1000; ++count) Sleep(1);
    CHECK(entered);
    Sleep(10); session.poll();
    CHECK(session.cancellingAfterTimeout());
    CHECK(session.busy());
    release = true; settle(session);
    REQUIRE(session.launchOutcome());
    REQUIRE(session.launchOutcome()->receipt);
    CHECK(session.launchOutcome()->receipt->pid == 42);
    CHECK(session.launchOutcome()->exceededBudget);
    CHECK_FALSE(session.launchOutcome()->error);
    const auto operation = session.launchOutcome()->operationId;
    session.requestRefresh(); settle(session);
    CHECK(session.launchOutcome()->operationId == operation);
    CHECK(session.operationResult().at("pid") == 42);
}

int main(int argc, char **argv) { return runDoctestMain(argc, argv); }
