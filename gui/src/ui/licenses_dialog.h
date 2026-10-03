#pragma once
#include <QDialog>
#include <QTimer>

class QLabel;
class QPlainTextEdit;
class QPushButton;

namespace guard {

class LicensesReader;

// Offline viewer for the embedded license set. One modeless window per
// MainWindow: repeated entry reuses and raises it, so at most one read is in
// flight. Copy is enabled only for a complete, successfully decoded text.
class LicensesDialog final : public QDialog {
    Q_OBJECT
public:
    explicit LicensesDialog(QWidget *parent = nullptr);
    // Dedicated test seam: injects the reader program under test.
    LicensesDialog(const QString &readerProgram, const QStringList &readerArguments, QWidget *parent = nullptr);

    LicensesReader *reader() const { return reader_; }
    void present();   // reuse/raise this window; (re)start an idle or failed read
    // QDialog::reject is a public slot; the override aborts any in-flight
    // read (Esc and window close route through here) before closing.
    void reject() override;

private:
    void render();
    void copy();

    LicensesReader *reader_;
    QLabel *status_;
    QPlainTextEdit *viewer_;
    QPushButton *copy_;
    QPushButton *retry_;
    QString loadedText_;   // last text pushed to the viewer
    QTimer feedback_;
};

}
