// Test-only licenses producer. No business code, no configuration, no real
// license files: scenarios emit deterministic stdout/stderr shapes the reader
// must survive. The body literal is duplicated in licenses_tests.cpp.
#include <QCoreApplication>
#include <QFile>
#include <QThread>
#include <cstdio>
#include <cstdlib>
#include <fcntl.h>
#include <io.h>

namespace {
const char kBody[] =
    "===== LICENSE =====\n"
    "Test license · 许可证 · ライセンス\n"
    "0123456789abcdefghijklmnopqrstuvwxyz\n"
    "-----\n\n"
    "===== THIRD_PARTY_NOTICES.md =====\n"
    "Test notice\n"
    "-----\n\n";

void writeFlushed(const char *data, size_t size) {
    std::fwrite(data, 1, size, stdout);
    std::fflush(stdout);
}
}

int main(int argc, char **argv) {
    QCoreApplication app(argc, argv);
    // The real `licenses` role writes raw bytes with WriteFile; match that
    // instead of the CRT's text-mode LF -> CRLF translation.
    _setmode(_fileno(stdout), _O_BINARY);
    _setmode(_fileno(stderr), _O_BINARY);
    const QString scenario = app.arguments().value(1);
    if (scenario == "ok") { writeFlushed(kBody, sizeof(kBody) - 1); return 0; }
    if (scenario == "split") {
        // Cut inside the three-byte UTF-8 sequence of 许 across two flushes.
        const QByteArray whole = QByteArray("===== LICENSE =====\nsplit:许可证\n");
        const int cut = whole.indexOf("许") + 1;
        writeFlushed(whole.left(cut).constData(), static_cast<size_t>(cut));
        QThread::msleep(50);
        writeFlushed(whole.mid(cut).constData(), static_cast<size_t>(whole.size() - cut));
        return 0;
    }
    if (scenario == "empty") return 0;
    if (scenario == "exit3") {
        writeFlushed(kBody, sizeof(kBody) - 1);
        std::fprintf(stderr, "boom detail\n");
        std::fflush(stderr);
        return 3;
    }
    if (scenario == "crash") { writeFlushed(kBody, sizeof(kBody) - 1); std::abort(); }
    if (scenario == "stderr-flood") {
        const QByteArray noise(256 * 1024, 'e');
        std::fwrite(noise.constData(), 1, static_cast<size_t>(noise.size()), stderr);
        std::fflush(stderr);
        writeFlushed(kBody, sizeof(kBody) - 1);
        return 0;
    }
    if (scenario == "oversize") {
        // 66 x 64 KiB exceeds the reader's 4 MiB stdout cap.
        const QByteArray chunk(64 * 1024, 'x');
        for (int written = 0; written < 66; ++written)
            writeFlushed(chunk.constData(), static_cast<size_t>(chunk.size()));
        return 0;
    }
    if (scenario == "hang") { writeFlushed(kBody, sizeof(kBody) - 1); for (;;) QThread::msleep(1000); }
    if (scenario == "late") { QThread::msleep(400); writeFlushed(kBody, sizeof(kBody) - 1); return 0; }
    if (scenario == "first-fail") {
        // Fails the first run, succeeds on retry; argv[2] is the marker file.
        const QString marker = app.arguments().value(2);
        if (marker.isEmpty()) return 64;
        if (QFile::exists(marker)) { writeFlushed(kBody, sizeof(kBody) - 1); return 0; }
        QFile file(marker);
        if (!file.open(QIODevice::WriteOnly) || !file.write("1")) return 64;
        std::fprintf(stderr, "first attempt fails\n");
        std::fflush(stderr);
        return 3;
    }
    return 64;
}
