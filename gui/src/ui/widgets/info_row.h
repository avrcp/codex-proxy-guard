#pragma once
#include <QLabel>
#include <QWidget>
class QPushButton;
namespace guard {
// A value label that never grows past its box. Long engine text (reused
// activation, unconfirmed outcomes) reaches ~180 characters, which would
// otherwise stretch the window past its 480x400 minimum; this elides instead.
class ElidedLabel final : public QLabel {
    Q_OBJECT
public:
    explicit ElidedLabel(QWidget *parent = nullptr);
    void setValue(const QString &text);
    // Row name used to prefix the accessible name, so values are announced
    // with their field ("Proxy value …") rather than on their own.
    void setRowLabel(const QString &label) { label_ = label; }
    // Overrides the value tooltip with an explanation (an error code, why an
    // action is blocked). Remembered across focus changes and cleared by the
    // next setValue().
    void setTooltip(const QString &text);
    // Expands to the full text in place, for keyboard and screen-reader users
    // who cannot hover a tooltip. Toggles back on a second activation.
    void toggleExpanded();
    bool isExpanded() const { return expanded_; }
signals:
    void expandedChanged(bool expanded);
protected:
    void resizeEvent(QResizeEvent *event) override;
    void keyPressEvent(QKeyEvent *event) override;
    void mousePressEvent(QMouseEvent *event) override;
    void focusInEvent(QFocusEvent *event) override;
    void focusOutEvent(QFocusEvent *event) override;
private:
    bool isTruncated() const;
    void applyElision();
    QString value_;
    QString label_;   // row name, prefixed to the accessible name
    QString tooltip_; // caller-supplied explanation, survives focus changes
    bool expanded_ = false;
};
class InfoRow final : public QWidget {
    Q_OBJECT
public:
    InfoRow(const QString &name, QWidget *parent = nullptr, QPushButton *action = nullptr);
    void setValue(const QString &value);
    void setTone(const QColor &color);
    // Explanation shown for the value (an error code, why an action is
    // blocked); forwarded to the value label, where hover and keyboard focus
    // actually land.
    void setTooltip(const QString &text);
    // The focusable value label, needed to place rows in the window tab chain.
    ElidedLabel *valueLabel() const { return value_; }
private:
    ElidedLabel *value_;
};
}
