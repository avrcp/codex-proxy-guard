#pragma once

#include <QByteArray>
#include <QJsonObject>
#include <QString>
#include <QVector>

namespace guard {

inline constexpr int ProtocolVersion = 1;
inline constexpr qsizetype MaxRequestBytes = 32 * 1024;
inline constexpr qsizetype MaxResponseBytes = 128 * 1024;
inline constexpr quint64 MaxProtocolId = 9007199254740991ULL;

bool protocolId(const QJsonValue &value, quint64 *id);
bool decodeMessage(const QByteArray &line, QJsonObject *object, QString *error);
QByteArray encodeRequest(quint64 id, const QString &method, const QJsonObject &params);

// Incremental framing keeps only one bounded, unfinished line. Failure is sticky.
class ProtocolDecoder {
public:
    bool feed(const QByteArray &bytes, QVector<QJsonObject> *messages, QString *error);
    bool finish(QString *error) const;
    void reset();

private:
    QByteArray buffer_;
    bool failed_ = false;
};

} // namespace guard
