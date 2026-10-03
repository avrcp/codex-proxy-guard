#include "licenses_reader.h"

#include <QCoreApplication>

namespace guard {
namespace {
// The embedded notice corpus measured ≈380 KiB at 0.6.1-rc.1 (LICENSE +
// THIRD_PARTY_NOTICES.md + the Qt license set). The stdout cap keeps >10x
// headroom so a legitimate corpus growth never truncates silently while
// memory stays bounded; exceeding it is an explicit failure.
constexpr qsizetype StdoutLimitBytes = 4 * 1024 * 1024;
constexpr qsizetype StderrLimitBytes = 64 * 1024;   // matches the bridge diagnostic limit
constexpr int StartupDeadlineMs = 10000;            // matches the bridge start deadline
constexpr int TotalDeadlineMs = 30000;              // streaming embedded resources is local I/O
constexpr int ReapWaitMs = 2000;                    // destructor fallback, teardown only

QString simplifiedExcerpt(const QByteArray &raw) {
    QString excerpt = QString::fromUtf8(raw).simplified();
    if (excerpt.size() > 160) excerpt.truncate(160);
    return excerpt;
}
}

LicensesReader::LicensesReader(QObject *parent)
    : LicensesReader(QCoreApplication::applicationFilePath(), {QStringLiteral("licenses")}, parent)
{
}

LicensesReader::LicensesReader(const QString &absoluteProgramPath, const QStringList &arguments, QObject *parent)
    : QObject(parent), program_(absoluteProgramPath), arguments_(arguments)
{
    process_.setProcessChannelMode(QProcess::SeparateChannels);
#ifdef Q_OS_WIN
    process_.setCreateProcessArgumentsModifier([](QProcess::CreateProcessArguments *args) {
        args->flags |= 0x08000000; // CREATE_NO_WINDOW; never create a shell or console.
    });
#endif
    deadline_.setSingleShot(true);
    connect(&deadline_, &QTimer::timeout, this, [this] {
        fail(QStringLiteral("The license texts were not produced within their time limit."));
    });
    connect(&process_, &QProcess::started, this, [this] { deadline_.start(TotalDeadlineMs); });
    connect(&process_, &QProcess::readyReadStandardOutput, this, &LicensesReader::readStdout);
    connect(&process_, &QProcess::readyReadStandardError, this, &LicensesReader::readStderr);
    connect(&process_, &QProcess::errorOccurred, this, [this](QProcess::ProcessError error) {
        if (error == QProcess::FailedToStart)
            fail(QStringLiteral("The license viewer process could not start."));
        // Crashed is classified by the finished handler's exit status.
        else if (error != QProcess::Crashed && !outcomeRecorded_)
            fail(QStringLiteral("Reading the license texts failed."));
    });
    connect(&process_, qOverload<int, QProcess::ExitStatus>(&QProcess::finished), this,
            &LicensesReader::finish);
}

LicensesReader::~LicensesReader()
{
    // Teardown-only fallback for a still-running child; window close cancels
    // asynchronously well before this. Only our own held child is reaped.
    if (process_.state() != QProcess::NotRunning) {
        stopChild();
        process_.waitForFinished(ReapWaitMs);
    }
}

void LicensesReader::start()
{
    // A previous attempt's stale finished delivery may still be pending after
    // fail() killed the child; never start a second process on top of it.
    if (state_ == State::Loading || process_.state() != QProcess::NotRunning) return;
    state_ = State::Loading;
    outcomeRecorded_ = false;
    stdout_.clear();
    stderr_.clear();
    text_.clear();
    errorText_.clear();
    emit stateChanged();
    process_.setProgram(program_);
    process_.setArguments(arguments_);
    deadline_.start(StartupDeadlineMs);
    process_.start();
}

void LicensesReader::cancel()
{
    if (state_ != State::Loading) return;
    fail(QStringLiteral("Loading was cancelled."));
}

void LicensesReader::readStdout()
{
    stdout_.append(process_.readAllStandardOutput());
    if (stdout_.size() > StdoutLimitBytes)
        fail(QStringLiteral("The license output exceeded its size limit."));
}

void LicensesReader::readStderr()
{
    stderr_.append(process_.readAllStandardError());
    if (stderr_.size() > StderrLimitBytes)    // keep the bounded tail only
        stderr_.remove(0, stderr_.size() - StderrLimitBytes);
}

void LicensesReader::finish(int exitCode, QProcess::ExitStatus exitStatus)
{
    readStdout();
    readStderr();
    if (outcomeRecorded_) return;    // failure, cancellation or overflow already decided the outcome
    deadline_.stop();
    if (exitStatus != QProcess::NormalExit) {
        fail(QStringLiteral("The license viewer process stopped abnormally."));
        return;
    }
    if (exitCode != 0) {
        fail(QStringLiteral("The license viewer exited with code %1.").arg(exitCode));
        return;
    }
    if (stdout_.isEmpty()) {
        fail(QStringLiteral("The license viewer produced no text."));
        return;
    }
    // Decode the complete buffer once; chunk boundaries can split multi-byte
    // characters, so per-chunk decoding would corrupt them.
    text_ = QString::fromUtf8(stdout_);
    stdout_.clear();
    state_ = State::Ready;
    emit stateChanged();
}

void LicensesReader::fail(const QString &message)
{
    if (outcomeRecorded_ || state_ != State::Loading) return;
    outcomeRecorded_ = true;
    const QString excerpt = simplifiedExcerpt(stderr_);
    errorText_ = excerpt.isEmpty() ? message : message + QStringLiteral("\n") + excerpt;
    text_.clear();
    stdout_.clear();
    state_ = State::Failed;
    emit stateChanged();
    stopChild();
}

void LicensesReader::stopChild()
{
    deadline_.stop();
    if (process_.state() == QProcess::NotRunning) return;
    // The headless licenses role owns no windows to close cooperatively, so the
    // held child handle is stopped directly. Nothing is ever matched by name:
    // GUI, bridge and activation worker share this executable file.
    process_.kill();
}

}
