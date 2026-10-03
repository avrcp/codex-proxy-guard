#include "startup_mode.h"

#include <QtTest>

using cpg::StartupRole;

class StartupModeTests final : public QObject {
    Q_OBJECT

private slots:
    void guiByDefault() {
        const auto mode = cpg::classifyStartup({});
        QCOMPARE(mode.role, StartupRole::Gui);
        QVERIFY(!mode.shouldExit);
        QVERIFY(!mode.smokeTest);
        QVERIFY(mode.configPath.isEmpty());
    }

    void guiWithConfig() {
        const auto spaced = cpg::classifyStartup({QStringLiteral("--config"),
                                                  QStringLiteral("C:/some dir/guard.toml")});
        QCOMPARE(spaced.role, StartupRole::Gui);
        QCOMPARE(spaced.configPath, QStringLiteral("C:/some dir/guard.toml"));
        const auto equals = cpg::classifyStartup({QStringLiteral("--config=C:/plain/guard.toml")});
        QCOMPARE(equals.role, StartupRole::Gui);
        QCOMPARE(equals.configPath, QStringLiteral("C:/plain/guard.toml"));
    }

    void guiSmokeTest() {
        const auto mode = cpg::classifyStartup({QStringLiteral("--smoke-test")});
        QCOMPARE(mode.role, StartupRole::Gui);
        QVERIFY(mode.smokeTest);
        const auto both = cpg::classifyStartup({QStringLiteral("--smoke-test"),
                                                QStringLiteral("--config"), QStringLiteral("g.toml")});
        QCOMPARE(both.role, StartupRole::Gui);
        QVERIFY(both.smokeTest);
        QCOMPARE(both.configPath, QStringLiteral("g.toml"));
    }

    void roleWordsInsideValuesNeverFlipTheRole() {
        // Option values are consumed as values: no positional command exists,
        // so none of these can be mistaken for the bridge role.
        const auto mode = cpg::classifyStartup({QStringLiteral("--config"),
                                                QStringLiteral("C:/tools/bridge/--smoke-test.toml")});
        QCOMPARE(mode.role, StartupRole::Gui);
        QCOMPARE(mode.configPath, QStringLiteral("C:/tools/bridge/--smoke-test.toml"));
    }

    void headlessCommands() {
        for (const QString &command : {QStringLiteral("bridge"), QStringLiteral("internal-activate-package"),
                                       QStringLiteral("build-info"), QStringLiteral("launch"),
                                       QStringLiteral("init-config"), QStringLiteral("config-path"),
                                       QStringLiteral("console"), QStringLiteral("licenses")}) {
            const auto mode = cpg::classifyStartup({command});
            QCOMPARE(mode.role, StartupRole::Cli);
            QVERIFY(!mode.shouldExit);
        }
    }

    void headlessGlobalFlags() {
        for (const QString &flag : {QStringLiteral("--build-info"), QStringLiteral("--help"),
                                    QStringLiteral("-h"), QStringLiteral("--version"),
                                    QStringLiteral("--json")}) {
            const auto mode = cpg::classifyStartup({flag});
            QCOMPARE(mode.role, StartupRole::Cli);
            QVERIFY(!mode.shouldExit);
        }
    }

    void bridgeWithConfig() {
        const auto mode = cpg::classifyStartup({QStringLiteral("--config"), QStringLiteral("g.toml"),
                                                QStringLiteral("bridge")});
        QCOMPARE(mode.role, StartupRole::Cli);
    }

    void separatorMakesPositionalArguments() {
        const auto mode = cpg::classifyStartup({QStringLiteral("--"), QStringLiteral("bridge")});
        QCOMPARE(mode.role, StartupRole::Cli);
        // A value expecting option consumes the next token verbatim, exactly
        // like QCommandLineParser: "--config --" makes "--" the value, and any
        // following token becomes a positional (and therefore a command).
        const auto consumed = cpg::classifyStartup({QStringLiteral("--config"),
                                                    QStringLiteral("--"), QStringLiteral("x.toml")});
        QVERIFY(consumed.shouldExit);
        QVERIFY(consumed.errorMessage.contains(QStringLiteral("Unknown command")));
    }

    void unknownCommandRejected() {
        const auto mode = cpg::classifyStartup({QStringLiteral("definitely-not-a-command")});
        QVERIFY(mode.shouldExit);
        QCOMPARE(mode.exitCode, 1);
        QVERIFY(mode.errorMessage.contains(QStringLiteral("CLI_INVALID")));
        const auto longName = cpg::classifyStartup({QString('x').repeated(5000)});
        QVERIFY(mode.shouldExit);
    }

    void contradictoryModesRejected() {
        QVERIFY(cpg::classifyStartup({QStringLiteral("bridge"), QStringLiteral("--smoke-test")}).shouldExit);
        QVERIFY(cpg::classifyStartup({QStringLiteral("--smoke-test"), QStringLiteral("--json")}).shouldExit);
        QVERIFY(cpg::classifyStartup({QStringLiteral("--config")}).shouldExit);
        QVERIFY(cpg::classifyStartup({QStringLiteral("--config"), QStringLiteral("a"),
                                      QStringLiteral("--config"), QStringLiteral("b")}).shouldExit);
    }
};

QTEST_GUILESS_MAIN(StartupModeTests)

#include "test_startup_mode.moc"
