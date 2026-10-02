// Test-only subprocess. No business operations or real Desktop activation.
#include <QCoreApplication>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QThread>
#include <cstdio>
#include <iostream>
#include <string>

static void output(const QJsonObject &object)
{
    const auto bytes = QJsonDocument(object).toJson(QJsonDocument::Compact) + '\n';
    std::fwrite(bytes.constData(), 1, static_cast<size_t>(bytes.size()), stdout);
    std::fflush(stdout);
}

int main(int argc, char **argv)
{
    QCoreApplication app(argc, argv);
    if (app.arguments().size() != 2 || app.arguments().at(1) != "bridge") return 2;
    bool operation = false;
    std::string line;
    while (std::getline(std::cin, line)) {
        if (line.size() > 32768) return 3;
        const auto request = QJsonDocument::fromJson(QByteArray::fromStdString(line)).object();
        const auto id = request.value("id");
        const auto method = request.value("method").toString();
        const auto params = request.value("params").toObject();
        const auto respond = [id](const QJsonObject &result) {
            output({{"schema", 1}, {"id", id}, {"ok", true}, {"result", result}});
        };
        if (method == "hello") {
            respond({{"protocol_version", 1}, {"engine_version", "test-only"},
                {"capabilities", QJsonArray{"snapshot", "set_proxy", "backend_proxy_consent", "launch", "repair", "cancel"}}});
        } else if (method == "snapshot") {
            respond({{"config_readiness", "ready"}});
        } else if (method == "set_proxy") {
            const auto scenario = params.value("host").toString();
            if (scenario == "crash") return 17;
            if (scenario == "hang") {
                respond({{"safe", true}});
                for (;;) QThread::msleep(1000);
            }
            if (scenario == "mismatch_hang") {
                output({{"schema", 1}, {"id", 900}, {"ok", true}, {"result", QJsonObject{}}});
                for (;;) QThread::msleep(1000);
            }
            if (scenario == "mismatch") {
                output({{"schema", 1}, {"id", 900}, {"ok", true}, {"result", QJsonObject{}}});
            } else if (scenario == "stderr") {
                const QByteArray diagnostic(192 * 1024, 'x');
                std::fwrite(diagnostic.constData(), 1, static_cast<size_t>(diagnostic.size()), stderr);
                std::fflush(stderr);
                respond({{"safe", true}});
            } else if (scenario == "oversize") {
                const QByteArray oversized(128 * 1024 + 1, 'x');
                std::fwrite(oversized.constData(), 1, static_cast<size_t>(oversized.size()), stdout);
                std::fflush(stdout);
            } else {
                output({{"schema", 1}, {"id", id}, {"ok", false},
                        {"error", QJsonObject{{"code", "CONFIG_INVALID"}, {"message", "Safe test error"}, {"retryable", false}}}});
            }
        } else if (method == "start_launch") {
            operation = true;
            respond({{"operation_id", 1}});
            output({{"schema", 1}, {"event", "operation_state"}, {"operation_id", 1},
                    {"state", "running"}, {"message", "Test-only operation"}});
        } else if (method == "cancel_operation" || method == "shutdown") {
            if (method == "shutdown") respond({});
            if (operation) {
                output({{"schema", 1}, {"event", "operation_finished"}, {"operation_id", 1},
                    {"ok", false}, {"error", QJsonObject{{"code", "CANCELLED"}, {"message", "Test cancellation"}}}});
                operation = false;
            }
            if (method == "cancel_operation") respond({});
            if (method == "shutdown") return 0;
        }
    }
    return 0;
}
