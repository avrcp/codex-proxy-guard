#include "licenses_dialog.h"
#include "engine/licenses_reader.h"
#include "theme.h"
#include <QApplication>
#include <QClipboard>
#include <QDialogButtonBox>
#include <QLabel>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QVBoxLayout>

namespace guard {

LicensesDialog::LicensesDialog(QWidget *parent)
    : LicensesDialog(QString(), QStringList(), parent)
{
}

LicensesDialog::LicensesDialog(const QString &readerProgram, const QStringList &readerArguments, QWidget *parent)
    : QDialog(parent)
{
    setWindowTitle("Licenses and third-party notices");
    setMinimumSize(460, 320);
    resize(640, 520);
    reader_ = readerProgram.isEmpty()
        ? new LicensesReader(this)
        : new LicensesReader(readerProgram, readerArguments, this);
    connect(reader_, &LicensesReader::stateChanged, this, &LicensesDialog::render);
    auto *layout = new QVBoxLayout(this);
    layout->setSpacing(10);
    status_ = new QLabel(this);
    status_->setProperty("muted", true);
    status_->setAccessibleName("License loading status");
    status_->setWordWrap(true);
    layout->addWidget(status_);
    viewer_ = new QPlainTextEdit(this);
    viewer_->setReadOnly(true);
    // Plain text only: license files are never interpreted as HTML/markup.
    viewer_->setAccessibleName("License texts, read only");
    layout->addWidget(viewer_, 1);
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Close, this);
    retry_ = buttons->addButton("Try again", QDialogButtonBox::ActionRole);
    copy_ = buttons->addButton("Copy all", QDialogButtonBox::ActionRole);
    connect(copy_, &QPushButton::clicked, this, &LicensesDialog::copy);
    connect(retry_, &QPushButton::clicked, this, [this] {
        reader_->start();   // allowed only from Failed; resets every previous attempt's data
        render();
    });
    connect(buttons, &QDialogButtonBox::rejected, this, &QDialog::reject);
    layout->addWidget(buttons);
    feedback_.setSingleShot(true);
    connect(&feedback_, &QTimer::timeout, this, &LicensesDialog::render);
    render();
}

void LicensesDialog::present()
{
    if (reader_->state() == LicensesReader::State::Idle || reader_->state() == LicensesReader::State::Failed)
        reader_->start();
    render();
    show();
    raise();
    activateWindow();
}

void LicensesDialog::reject()
{
    // Esc / window close during a read aborts our own child bounded; no
    // dangling callback can update this dialog afterwards.
    reader_->cancel();
    feedback_.stop();
    QDialog::reject();
}

void LicensesDialog::copy()
{
    if (reader_->state() != LicensesReader::State::Ready) return;
    QApplication::clipboard()->setText(reader_->text());
    status_->setText("Copied the complete license text to the clipboard.");
    feedback_.start(1500);   // then render() restores the regular status line
}

void LicensesDialog::render()
{
    switch (reader_->state()) {
    case LicensesReader::State::Idle:
    case LicensesReader::State::Loading:
        setDynamicProperty(status_, "error", false);
        status_->setText("Loading embedded license texts…");
        retry_->setVisible(false);
        copy_->setEnabled(false);
        if (!loadedText_.isEmpty()) { viewer_->setPlainText(QString()); loadedText_.clear(); }
        break;
    case LicensesReader::State::Ready: {
        const QString text = reader_->text();
        setDynamicProperty(status_, "error", false);
        status_->setText(QStringLiteral("Loaded %1 characters.").arg(QString::number(text.size())));
        if (text != loadedText_) { viewer_->setPlainText(text); loadedText_ = text; }
        retry_->setVisible(false);
        copy_->setEnabled(!text.isEmpty());
        break;
    }
    case LicensesReader::State::Failed:
        setDynamicProperty(status_, "error", true);
        status_->setText(reader_->errorText());
        if (!loadedText_.isEmpty()) { viewer_->setPlainText(QString()); loadedText_.clear(); }
        retry_->setVisible(true);
        copy_->setEnabled(false);
        break;
    }
}

}
