// Behaviour tests for the offline licenses viewer (reader + dialog). All
// scenarios run the fake_licenses double — the real EXE stream is covered by
// backend/tests. IDs L01–L11 refer to the acceptance matrix in
// docs/SINGLE_EXE_ACCEPTANCE.md.
#include "engine/licenses_reader.h"
#include "ui/licenses_dialog.h"
#include <QApplication>
#include <QClipboard>
#include <QCoreApplication>
#include <QDir>
#include <QLabel>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QtTest>
#include <memory>

using namespace guard;
namespace {
QString fakePath() {
    return QDir(QCoreApplication::applicationDirPath()).absoluteFilePath("fake_licenses.exe");
}
// Duplicated from gui/tests/fake_licenses.cpp: the double and the expectation
// are reviewed together.
const char kBody[] =
    "===== LICENSE =====\n"
    "Test license · 许可证 · ライセンス\n"
    "0123456789abcdefghijklmnopqrstuvwxyz\n"
    "-----\n\n"
    "===== THIRD_PARTY_NOTICES.md =====\n"
    "Test notice\n"
    "-----\n\n";
QString bodyText() { return QString::fromUtf8(kBody); }
std::unique_ptr<LicensesReader> makeReader(const QString &scenario, const QString &extra = {}) {
    QStringList arguments{scenario};
    if (!extra.isEmpty()) arguments << extra;
    return std::make_unique<LicensesReader>(fakePath(), arguments);
}
QPushButton *button(QDialog *dialog, const char *text) {
    for (auto *candidate : dialog->findChildren<QPushButton *>())
        if (candidate->text() == QLatin1String(text)) return candidate;
    return nullptr;
}
}

class LicensesTests : public QObject {
    Q_OBJECT
private slots:
    void l01CompleteDecodedTextMatchesProducer() {
        auto reader = makeReader(QStringLiteral("ok"));
        QSignalSpy changed(reader.get(), &LicensesReader::stateChanged);
        reader->start();
        QVERIFY(reader->isLoading());
        QTRY_COMPARE_WITH_TIMEOUT(reader->state(), LicensesReader::State::Ready, 5000);
        QCOMPARE(reader->text(), bodyText());
        QCOMPARE(changed.size(), qsizetype(2));   // Loading -> Ready, nothing else
        QVERIFY(!reader->isChildRunning());
    }
    void l05SplitMultibyteCharactersStayIntact() {
        auto reader = makeReader(QStringLiteral("split"));
        reader->start();
        QTRY_COMPARE_WITH_TIMEOUT(reader->state(), LicensesReader::State::Ready, 5000);
        const QString expected = QStringLiteral("===== LICENSE =====\nsplit:许可证\n");
        QCOMPARE(reader->text(), expected);
        QVERIFY(!reader->text().contains(QChar(0xFFFD)));
    }
    void l02EmptySuccessIsAnExplicitError() {
        auto reader = makeReader(QStringLiteral("empty"));
        reader->start();
        QTRY_COMPARE_WITH_TIMEOUT(reader->state(), LicensesReader::State::Failed, 5000);
        QVERIFY(reader->text().isEmpty());
        QVERIFY(reader->errorText().contains("no text"));
    }
    void l03StartFailureIsReported() {
        QTemporaryDir dir;
        const QStringList arguments{QStringLiteral("ok")};
        auto reader = std::make_unique<LicensesReader>(dir.filePath("missing.exe"), arguments);
        reader->start();
        QTRY_COMPARE_WITH_TIMEOUT(reader->state(), LicensesReader::State::Failed, 5000);
        QVERIFY(reader->errorText().contains("could not start"));
        QVERIFY(!reader->isChildRunning());
    }
    void l04NonzeroExitAndCrashAreFailures() {
        auto exited = makeReader(QStringLiteral("exit3"));
        exited->start();
        QTRY_COMPARE_WITH_TIMEOUT(exited->state(), LicensesReader::State::Failed, 5000);
        QVERIFY(exited->text().isEmpty());   // partial stdout is never shown as success
        QVERIFY(exited->errorText().contains("code 3"));
        QVERIFY(exited->errorText().contains("boom detail"));
        auto crashed = makeReader(QStringLiteral("crash"));
        crashed->start();
        QTRY_COMPARE_WITH_TIMEOUT(crashed->state(), LicensesReader::State::Failed, 5000);
        QVERIFY(crashed->errorText().contains("abnormally"));
    }
    void l06OutputAndDiagnosticsStayBounded() {
        auto oversized = makeReader(QStringLiteral("oversize"));
        oversized->start();
        QTRY_COMPARE_WITH_TIMEOUT(oversized->state(), LicensesReader::State::Failed, 10000);
        QVERIFY(oversized->errorText().contains("size limit"));
        QTRY_VERIFY_WITH_TIMEOUT(!oversized->isChildRunning(), 5000);
        auto flooded = makeReader(QStringLiteral("stderr-flood"));
        flooded->start();
        QTRY_COMPARE_WITH_TIMEOUT(flooded->state(), LicensesReader::State::Ready, 5000);
        QCOMPARE(flooded->text(), bodyText());   // stderr never pollutes the text
    }
    void l07HangingChildHitsDeadlineAndIsStopped() {
        auto reader = makeReader(QStringLiteral("hang"));
        reader->start();
        QVERIFY(reader->isLoading());
        QTRY_COMPARE_WITH_TIMEOUT(reader->state(), LicensesReader::State::Failed, 35000);
        QVERIFY(reader->errorText().contains("time limit"));
        QTRY_VERIFY_WITH_TIMEOUT(!reader->isChildRunning(), 5000);
    }
    void l11RetryAfterFailureUsesFreshState() {
        QTemporaryDir dir;
        auto reader = makeReader(QStringLiteral("first-fail"), dir.filePath("marker"));
        reader->start();
        QTRY_COMPARE_WITH_TIMEOUT(reader->state(), LicensesReader::State::Failed, 5000);
        QVERIFY(reader->errorText().contains("code 3"));
        reader->start();
        QVERIFY(reader->isLoading());
        QTRY_COMPARE_WITH_TIMEOUT(reader->state(), LicensesReader::State::Ready, 5000);
        QVERIFY(reader->errorText().isEmpty());
        QCOMPARE(reader->text(), bodyText());
    }
    void l08DialogLoadingReadyAndCopyFlow() {
        LicensesDialog dialog(fakePath(), {QStringLiteral("late")});
        dialog.present();
        QVERIFY(dialog.reader()->isLoading());
        auto *copy = button(&dialog, "Copy all");
        auto *status = dialog.findChild<QLabel *>();
        auto *viewer = dialog.findChild<QPlainTextEdit *>();
        QVERIFY(copy && status && viewer);
        QVERIFY(!copy->isEnabled());
        QVERIFY(status->text().contains("Loading"));
        QTRY_VERIFY_WITH_TIMEOUT(copy->isEnabled(), 5000);
        QCOMPARE(viewer->toPlainText(), bodyText());
        QVERIFY(status->text().contains("Loaded"));
        copy->click();
        QCOMPARE(QApplication::clipboard()->text(), bodyText());
        QVERIFY(status->text().contains("Copied"));
        QTRY_VERIFY_WITH_TIMEOUT(status->text().contains("Loaded"), 4000);   // feedback is brief
    }
    void l08RepeatedPresentKeepsASingleAttempt() {
        LicensesDialog dialog(fakePath(), {QStringLiteral("late")});
        QSignalSpy changed(dialog.reader(), &LicensesReader::stateChanged);
        dialog.present();
        dialog.present();
        dialog.present();
        QTRY_COMPARE_WITH_TIMEOUT(dialog.reader()->state(), LicensesReader::State::Ready, 5000);
        QCOMPARE(changed.size(), qsizetype(2));   // one Loading -> Ready cycle
        QVERIFY(!dialog.reader()->isChildRunning());
    }
    void l09RejectDuringLoadCancelsAndReapsTheChild() {
        LicensesDialog dialog(fakePath(), {QStringLiteral("hang")});
        dialog.present();
        QTRY_VERIFY_WITH_TIMEOUT(dialog.reader()->isChildRunning(), 5000);
        dialog.reject();   // what Esc and window close trigger
        QCOMPARE(dialog.reader()->state(), LicensesReader::State::Failed);
        QVERIFY(dialog.reader()->errorText().contains("cancelled"));
        QTRY_VERIFY_WITH_TIMEOUT(!dialog.reader()->isChildRunning(), 5000);
    }
    void l11DialogRetryResetsStaleContent() {
        QTemporaryDir dir;
        LicensesDialog dialog(fakePath(), {QStringLiteral("first-fail"), dir.filePath("marker")});
        dialog.present();
        QTRY_COMPARE_WITH_TIMEOUT(dialog.reader()->state(), LicensesReader::State::Failed, 5000);
        auto *viewer = dialog.findChild<QPlainTextEdit *>();
        QVERIFY(viewer->toPlainText().isEmpty());   // failed text is never left behind
        auto *retry = button(&dialog, "Try again");
        QVERIFY(retry && retry->isVisible());
        retry->click();
        QVERIFY(dialog.reader()->isLoading());
        QTRY_COMPARE_WITH_TIMEOUT(dialog.reader()->state(), LicensesReader::State::Ready, 5000);
        QVERIFY(dialog.reader()->errorText().isEmpty());
        QCOMPARE(viewer->toPlainText(), bodyText());
    }
};

QTEST_MAIN(LicensesTests)
#include "licenses_tests.moc"
