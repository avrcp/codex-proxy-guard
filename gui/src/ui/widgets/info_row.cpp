#include "info_row.h"
#include <QHBoxLayout>
#include <QPushButton>
#include <QResizeEvent>
namespace guard {
ElidedLabel::ElidedLabel(QWidget *parent) : QLabel(parent) {
    setTextFormat(Qt::PlainText);
    setSizePolicy(QSizePolicy::Ignored, QSizePolicy::Preferred);
    setMinimumWidth(40);
}
void ElidedLabel::setValue(const QString &text) {
    value_ = text;
    setToolTip(text);
    setAccessibleName(text);
    setText(fontMetrics().elidedText(text, Qt::ElideMiddle, width()));
}
void ElidedLabel::resizeEvent(QResizeEvent *event) {
    QLabel::resizeEvent(event);
    setText(fontMetrics().elidedText(value_, Qt::ElideMiddle, width()));
}
InfoRow::InfoRow(const QString &name, QWidget *parent, QPushButton *action) : QWidget(parent) {
    auto *layout = new QHBoxLayout(this);
    layout->setContentsMargins(12, 3, 12, 3);
    layout->setSpacing(10);
    auto *label = new QLabel(name, this);
    label->setProperty("muted", true);
    label->setFixedWidth(68);
    value_ = new ElidedLabel(this);
    layout->addWidget(label);
    layout->addWidget(value_, 1);
    if (action) layout->addWidget(action);
    setMinimumHeight(action ? 44 : 28);
    setAccessibleName(name);
}
void InfoRow::setValue(const QString &value) { value_->setValue(value); }
void InfoRow::setTone(const QColor &color) { value_->setStyleSheet("color:" + color.name()); }
}
