#pragma once
#include <QColor>
class QApplication;
namespace guard {
struct ThemeTokens {
    QColor window, surface, hover, border, text, secondary, accent, success, warning, danger;
    static ThemeTokens system();
};
void applyTheme(QApplication &app);
}
