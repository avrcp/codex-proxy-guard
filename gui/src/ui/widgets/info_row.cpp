#include "info_row.h"
#include <QFontMetrics>
#include <QHBoxLayout>
#include <QKeyEvent>
#include <QMouseEvent>
#include <QPushButton>
#include <QResizeEvent>
namespace guard {
namespace {
// Two lines is the most DESIGN.md's 480x400 minimum absorbs without the window
// growing; a single line hid the second half of every long engine message.
constexpr int kMaxLines = 2;
}
ElidedLabel::ElidedLabel(QWidget *parent) : QLabel(parent) {
    setTextFormat(Qt::PlainText);
    setWordWrap(true);
    setSizePolicy(QSizePolicy::Ignored, QSizePolicy::Preferred);
    setMinimumWidth(40);
    // Focusable so the full text is reachable by keyboard; without this the
    // tooltip was the only route to it.
    setFocusPolicy(Qt::StrongFocus);
    connect(this, &ElidedLabel::expandedChanged, this, [this] { applyElision(); });
}
bool ElidedLabel::isTruncated() const {
    if (value_.isEmpty()) return false;
    const auto bounds = QRect(0, 0, qMax(1, width()), kMaxLines * fontMetrics().lineSpacing());
    return fontMetrics().boundingRect(bounds, Qt::TextWordWrap, value_).height() > bounds.height();
}
void ElidedLabel::setValue(const QString &text) {
    value_ = text;
    // A new value invalidates any caller tooltip from the previous one; the
    // caller re-sets what it needs right after, as render() does.
    tooltip_.clear();
    QLabel::setToolTip(text);
    // Keeps the row name so a screen reader announces "Proxy value …", not a
    // bare value with no context.
    setAccessibleName(label_.isEmpty() ? text : label_ + QLatin1String(" value. ") + text);
    if (expanded_) { expanded_ = false; emit expandedChanged(false); }
    applyElision();
}
void ElidedLabel::setTooltip(const QString &text) {
    tooltip_ = text;
    QLabel::setToolTip(text);
}
void ElidedLabel::toggleExpanded() {
    if (value_.isEmpty()) return;
    expanded_ = !expanded_;
    emit expandedChanged(expanded_);
}
void ElidedLabel::applyElision() {
    if (expanded_ || !isTruncated()) { setText(value_); setCursor(Qt::ArrowCursor); return; }
    // ElideMiddle keeps both ends readable, which matters for paths, versions
    // and URLs whose tail carries the distinguishing part.
    setText(fontMetrics().elidedText(value_, Qt::ElideMiddle, width() * kMaxLines));
    setCursor(Qt::PointingHandCursor);
}
void ElidedLabel::resizeEvent(QResizeEvent *event) {
    QLabel::resizeEvent(event);
    applyElision();
}
void ElidedLabel::keyPressEvent(QKeyEvent *event) {
    // Space and Enter expand/collapse, matching the focused-button convention
    // the window already uses for Enter.
    if (event->key() == Qt::Key_Space || event->key() == Qt::Key_Return || event->key() == Qt::Key_Enter) {
        toggleExpanded();
        event->accept();
        return;
    }
    if (event->key() == Qt::Key_Escape && expanded_) {
        expanded_ = false;
        emit expandedChanged(false);
        event->accept();
        return;
    }
    QLabel::keyPressEvent(event);
}
void ElidedLabel::mousePressEvent(QMouseEvent *event) {
    // Left click expands a truncated value; other buttons keep default
    // handling so they cannot toggle by accident.
    if (event->button() == Qt::LeftButton) toggleExpanded();
    QLabel::mousePressEvent(event);
}
void ElidedLabel::focusInEvent(QFocusEvent *event) {
    QLabel::focusInEvent(event);
    // Only annotate when no caller-supplied explanation exists; a real
    // explanation must not be replaced by the keyboard hint.
    if (isTruncated() && tooltip_.isEmpty())
        QLabel::setToolTip(QStringLiteral("%1\n(Press Space or Enter to show the full text)").arg(value_));
}
void ElidedLabel::focusOutEvent(QFocusEvent *event) {
    QLabel::focusOutEvent(event);
    QLabel::setToolTip(tooltip_.isEmpty() ? value_ : tooltip_);
    if (expanded_) { expanded_ = false; emit expandedChanged(false); }
}
InfoRow::InfoRow(const QString &name, QWidget *parent, QPushButton *action) : QWidget(parent) {
    auto *layout = new QHBoxLayout(this);
    layout->setContentsMargins(12, 3, 12, 3);
    layout->setSpacing(10);
    auto *label = new QLabel(name, this);
    label->setProperty("muted", true);
    label->setFixedWidth(68);
    value_ = new ElidedLabel(this);
    value_->setRowLabel(name);
    layout->addWidget(label);
    layout->addWidget(value_, 1);
    if (action) layout->addWidget(action);
    setMinimumHeight(action ? 44 : 28);
    setAccessibleName(name);
}
void InfoRow::setValue(const QString &value) { value_->setValue(value); }
void InfoRow::setTone(const QColor &color) { value_->setStyleSheet("color:" + color.name()); }
void InfoRow::setTooltip(const QString &text) { value_->setTooltip(text); }
}
