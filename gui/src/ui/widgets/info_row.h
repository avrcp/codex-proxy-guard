#pragma once
#include <QLabel>
#include <QWidget>
class QPushButton;
namespace guard {
class ElidedLabel final : public QLabel {
public:
    explicit ElidedLabel(QWidget *parent = nullptr);
    void setValue(const QString &text);
protected:
    void resizeEvent(QResizeEvent *event) override;
private:
    QString value_;
};
class InfoRow final : public QWidget {
public:
    InfoRow(const QString &name, QWidget *parent = nullptr, QPushButton *action = nullptr);
    void setValue(const QString &value);
    void setTone(const QColor &color);
private:
    ElidedLabel *value_;
};
}
