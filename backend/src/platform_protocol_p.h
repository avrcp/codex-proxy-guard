#pragma once
#include <QByteArray>
#include <QJsonObject>

namespace cpg::detail {
// Pure protocol validators; exposed privately for side-effect-free fixtures.
QJsonObject activationRequest(const QByteArray &bytes);
QJsonObject activationReceipt(const QByteArray &bytes);
}
