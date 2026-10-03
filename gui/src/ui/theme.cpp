#include "theme.h"
#include <QApplication>
#include <QFontDatabase>
#include <QPalette>
#include <QStyle>
#include <QStyleHints>
#include <QWidget>
namespace guard {
// `border` carries structural meaning (control outlines, card edge, progress
// track) and must clear WCAG 2.x 3:1 as a non-text boundary on every surface
// it lands on. The previous dark #405365 managed only 1.74:1 on `surface` and
// the light #b9c9d5 only 1.70:1, so outlines vanished in both schemes.
// Measured with the sRGB relative-luminance formula (CR = (L1+0.05)/(L2+0.05)):
// dark  #6a88a3 -> surface 3.74, window 4.39, hover 3.11
// light #6b7f8f -> surface 4.15, window 3.86, hover 3.57
ThemeTokens ThemeTokens::system() {
    if (QGuiApplication::styleHints()->colorScheme() == Qt::ColorScheme::Dark)
        return {"#17212b", "#202e3b", "#293b4b", "#6a88a3", "#eef4f8", "#b2c3d1", "#69b9ed", "#83d7ad", "#efc276", "#ffabaf"};
    return {"#f4f7fa", "#ffffff", "#e6eff6", "#6b7f8f", "#182f40", "#506777", "#176397", "#216841", "#84500a", "#b42339"};
}
void applyTheme(QApplication &app) {
    const auto t = ThemeTokens::system();
    QPalette palette;
    palette.setColor(QPalette::Window, t.window);
    palette.setColor(QPalette::WindowText, t.text);
    palette.setColor(QPalette::Base, t.surface);
    palette.setColor(QPalette::Text, t.text);
    palette.setColor(QPalette::Button, t.surface);
    palette.setColor(QPalette::ButtonText, t.text);
    palette.setColor(QPalette::Highlight, t.accent);
    palette.setColor(QPalette::HighlightedText, t.window);
    app.setPalette(palette);
    QFont font(QFontDatabase::families().contains("Segoe UI Variable") ? "Segoe UI Variable" : "Segoe UI");
    font.setPointSize(10);
    app.setFont(font);
    // Built by concatenation rather than a %10 placeholder: QString::arg tops
    // out at nine string arguments, and `danger` is the tenth token.
    // A failure must not look like an idle note: every status surface sets
    // property("error", true). The danger token measures 7.72 / 6.48 on
    // `surface` while muted secondary sits at 7.67 / 5.92, so the two states
    // now differ by hue, not by wording alone.
    // Sizes are declared in pt, not px, so they scale with the user's DPI and
    // text-zoom setting together with the widget font; three px sizes in a row
    // ignored both.
    const auto errorRule = QStringLiteral("QLabel[error=\"true\"] { color:%1; }").arg(t.danger.name());
    app.setStyleSheet(QStringLiteral(
        "QWidget { color:%1; } QMainWindow, QDialog { background:%2; }"
        "QLabel[muted=\"true\"] { color:%3; }"
        "QLabel#heading { font-size:15pt; font-weight:600; }"
        // 8.5pt (~11.3px) keeps the app's most important signal above the
        // 11px small-type floor; at the old 10px it was smaller than the body
        // text it sits beside. Letter spacing is not a QSS property — it is
        // applied on the badge widget itself via QFont in MainWindow.
        "QLabel#badge { padding:4px 9px; border:1px solid %4; border-radius:10px; font-size:8.5pt; font-weight:600; }"
        // Secondary buttons reach a 40px hit area (min-height 26px + 12px
        // vertical padding); 20px left a ~32px target, under the 44px guidance.
        "QPushButton { background:%5; border:1px solid %4; border-radius:6px; padding:6px 11px; min-height:26px; }"
        "QPushButton:hover { background:%6; } QPushButton:pressed { border-color:%7; }"
        "QPushButton:focus { border:2px solid %7; padding:5px 10px; }"
        "QPushButton:disabled { color:%3; background:%2; border-color:%4; }"
        "QPushButton#launch:enabled { background:%7; color:%2; border-color:%7; font-weight:600; min-height:28px; }"
        "QPushButton#launch:enabled:hover { background:%1; }"
        "QPushButton#launch:focus { border:2px solid %2; }"
        "QLineEdit,QSpinBox { background:%5; border:1px solid %4; border-radius:4px; padding:6px; }"
        "QLineEdit:focus,QSpinBox:focus { border:2px solid %7; padding:5px; }"
        "QFrame#rows { background:%5; border:1px solid %4; border-radius:8px; }"
        // The chunk is `text`, not `accent`: once `border` was raised to a
        // compliant 3:1 outline, an accent chunk on a border track measured
        // 1.72 / 1.55 and the bar stopped reading as moving. text-on-border
        // measures 3.34 / 3.33 while the track stays visible on `window`
        // (4.39 / 3.86), so the filled extent survives both schemes.
        "QProgressBar { max-height:3px; border:0; background:%4; } QProgressBar::chunk { background:%1; }"
        // Focusable value labels need a visible focus ring too (they expand on
        // Space/Enter); the idle padding reserves the 1px so text does not
        // shift when the border appears.
        "ElidedLabel { padding:1px; }"
        "ElidedLabel:focus { border:1px solid %7; border-radius:3px; padding:0px; }"
    ).arg(t.text.name(), t.window.name(), t.secondary.name(), t.border.name(), t.surface.name(), t.hover.name(), t.accent.name())
        + errorRule);
}
void setDynamicProperty(QWidget *widget, const char *name, bool value) {
    if (widget->property(name).toBool() == value) return;
    widget->setProperty(name, value);
    widget->style()->unpolish(widget);
    widget->style()->polish(widget);
}
}
