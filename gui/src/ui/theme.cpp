#include "theme.h"
#include <QApplication>
#include <QFontDatabase>
#include <QPalette>
#include <QStyleHints>
namespace guard {
ThemeTokens ThemeTokens::system() {
    if (QGuiApplication::styleHints()->colorScheme() == Qt::ColorScheme::Dark)
        return {"#17212b", "#202e3b", "#293b4b", "#405365", "#eef4f8", "#b2c3d1", "#69b9ed", "#83d7ad", "#efc276", "#ffabaf"};
    return {"#f4f7fa", "#ffffff", "#e6eff6", "#b9c9d5", "#182f40", "#506777", "#176397", "#216841", "#84500a", "#b42339"};
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
    app.setStyleSheet(QStringLiteral(
        "QWidget { color:%1; } QMainWindow, QDialog { background:%2; }"
        "QLabel[muted=\"true\"] { color:%3; }"
        "QLabel#heading { font-size:20px; font-weight:600; }"
        "QLabel#badge { padding:4px 9px; border:1px solid %4; border-radius:10px; font-size:10px; font-weight:600; }"
        "QPushButton { background:%5; border:1px solid %4; border-radius:6px; padding:6px 11px; min-height:20px; }"
        "QPushButton:hover { background:%6; } QPushButton:pressed { border-color:%7; }"
        "QPushButton:focus { border:2px solid %7; padding:5px 10px; }"
        "QPushButton:disabled { color:%3; background:%2; border-color:%4; }"
        "QPushButton#launch:enabled { background:%7; color:%2; border-color:%7; font-weight:600; min-height:28px; }"
        "QPushButton#launch:enabled:hover { background:%1; }"
        "QPushButton#launch:focus { border:2px solid %1; }"
        "QLineEdit,QSpinBox { background:%5; border:1px solid %4; border-radius:4px; padding:6px; }"
        "QLineEdit:focus,QSpinBox:focus { border-color:%7; }"
        "QFrame#rows { background:%5; border-radius:8px; }"
        "QProgressBar { max-height:3px; border:0; background:%4; } QProgressBar::chunk { background:%7; }"
    ).arg(t.text.name(), t.window.name(), t.secondary.name(), t.border.name(), t.surface.name(), t.hover.name(), t.accent.name()));
}
}
