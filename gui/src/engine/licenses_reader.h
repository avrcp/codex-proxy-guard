#pragma once
#include <QObject>
#include <QProcess>
#include <QStringList>
#include <QTimer>

namespace guard {

// Streams the embedded license set by running this executable's headless
// `licenses` role as a short-lived Guard-owned child — the same notice stream
// the CLI prints, with no bridge frame, configuration or network dependency.
// Output is collected as raw bytes under an explicit cap and decoded once,
// after the child finished, so multi-byte characters split across pipe chunks
// stay intact. All waiting is signal-driven; deadlines stop only the child
// process held by this object, never anything matched by name.
class LicensesReader final : public QObject {
    Q_OBJECT
public:
    enum class State { Idle, Loading, Ready, Failed };

    // Production entry: always this executable file, always the verified
    // `licenses` argument — program and arguments stay separated, no shell.
    explicit LicensesReader(QObject *parent = nullptr);
    // Dedicated test seam: an absolute program path with explicit arguments.
    LicensesReader(const QString &absoluteProgramPath, const QStringList &arguments, QObject *parent = nullptr);
    ~LicensesReader() override;

    void start();   // one attempt at a time; ignored while an attempt is active
    void cancel();  // user abort: bounded stop of our own child, ends Failed
    State state() const { return state_; }
    bool isLoading() const { return state_ == State::Loading; }
    bool isChildRunning() const { return process_.state() != QProcess::NotRunning; }
    QString text() const { return text_; }       // complete decoded text, empty unless Ready
    QString errorText() const { return errorText_; }

signals:
    void stateChanged();  // emitted whenever state, text or errorText changed

private:
    void fail(const QString &message);
    void stopChild();
    void readStdout();
    void readStderr();
    void finish(int exitCode, QProcess::ExitStatus exitStatus);

    QString program_;
    QStringList arguments_;
    QProcess process_;
    QTimer deadline_;
    State state_ = State::Idle;
    bool outcomeRecorded_ = false;
    QByteArray stdout_;
    QByteArray stderr_;   // bounded tail, for failure context only
    QString text_, errorText_;
};

}
