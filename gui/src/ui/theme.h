#pragma once
#include <QColor>
class QApplication;
class QWidget;
namespace guard {
struct ThemeTokens {
    QColor window, surface, hover, border, text, secondary, accent, success, warning, danger;
    static ThemeTokens system();
};
void applyTheme(QApplication &app);
// Toggles a style-rule driven by a dynamic property (e.g. the danger-toned
// error state) and performs the repolish dynamic properties need to take
// effect; a no-op when the property already holds the value.
void setDynamicProperty(QWidget *widget, const char *name, bool value);
}
