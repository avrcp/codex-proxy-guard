#include "protocol.h"

#include <QJsonDocument>
#include <QJsonParseError>
#include <QStringDecoder>
#include <cmath>

namespace guard {

bool protocolId(const QJsonValue &value, quint64 *id)
{
    if (!value.isDouble()) return false;
    const double number = value.toDouble();
    if (!std::isfinite(number) || number < 1 || number > static_cast<double>(MaxProtocolId)
        || std::floor(number) != number) return false;
    *id = static_cast<quint64>(number);
    return true;
}

static bool validError(const QJsonValue &value)
{
    if (!value.isObject()) return false;
    const auto error = value.toObject();
    return error.value("code").isString() && !error.value("code").toString().isEmpty()
        && error.value("message").isString();
}

bool decodeMessage(const QByteArray &line, QJsonObject *object, QString *error)
{
    const auto reject = [error](const QString &message) { *error = message; return false; };
    if (line.isEmpty() || line.size() > MaxResponseBytes)
        return reject(QStringLiteral("Engine response exceeds the protocol boundary."));
    QStringDecoder utf8(QStringDecoder::Utf8);
    const QString decoded = utf8.decode(line);
    // Round-trip also detects a truncated final UTF-8 sequence held by the decoder.
    if (utf8.hasError() || decoded.toUtf8() != line)
        return reject(QStringLiteral("Engine response is not valid UTF-8."));
    QJsonParseError parseError;
    const auto document = QJsonDocument::fromJson(line, &parseError);
    if (parseError.error != QJsonParseError::NoError || !document.isObject())
        return reject(QStringLiteral("Engine response is not one JSON object."));
    const auto message = document.object();
    const auto schema = message.value("schema");
    if (!schema.isDouble() || schema.toDouble() != ProtocolVersion)
        return reject(QStringLiteral("Engine / GUI version mismatch."));
    quint64 id = 0;
    if (message.contains("event")) {
        if (message.contains("id") || !message.value("event").isString()
            || !protocolId(message.value("operation_id"), &id))
            return reject(QStringLiteral("Engine event has an invalid identity."));
        const auto kind = message.value("event").toString();
        if (kind == "operation_state") {
            if (!message.value("state").isString() || !message.value("message").isString())
                return reject(QStringLiteral("Engine progress event is invalid."));
        } else if (kind == "operation_finished") {
            if (!message.value("ok").isBool()
                || (message.value("ok").toBool() ? !message.value("result").isObject()
                                                : !validError(message.value("error"))))
                return reject(QStringLiteral("Engine completion event is invalid."));
        } else {
            return reject(QStringLiteral("Engine event is unsupported."));
        }
    } else if (!protocolId(message.value("id"), &id) || !message.value("ok").isBool()
               || (message.value("ok").toBool() ? !message.value("result").isObject()
                                               : !validError(message.value("error")))) {
        return reject(QStringLiteral("Engine response envelope is invalid."));
    }
    *object = message;
    return true;
}

QByteArray encodeRequest(quint64 id, const QString &method, const QJsonObject &params)
{
    if (id == 0 || id > MaxProtocolId || method.isEmpty()) return {};
    auto bytes = QJsonDocument(QJsonObject{{"schema", ProtocolVersion},
        {"id", static_cast<qint64>(id)}, {"method", method}, {"params", params}})
        .toJson(QJsonDocument::Compact);
    if (bytes.size() > MaxRequestBytes) return {};
    bytes.append('\n');
    return bytes;
}

bool ProtocolDecoder::feed(const QByteArray &bytes, QVector<QJsonObject> *messages, QString *error)
{
    if (failed_) { *error = QStringLiteral("Engine protocol is unavailable."); return false; }
    qsizetype offset = 0;
    while (offset < bytes.size()) {
        const qsizetype end = bytes.indexOf('\n', offset);
        const qsizetype count = end < 0 ? bytes.size() - offset : end - offset;
        if (buffer_.size() + count > MaxResponseBytes) {
            failed_ = true;
            buffer_.clear();
            *error = QStringLiteral("Engine response exceeds 128 KiB.");
            return false;
        }
        buffer_.append(bytes.constData() + offset, count);
        if (end < 0) break;
        // Accept the conventional CRLF transport spelling without changing JSON content.
        if (buffer_.endsWith('\r')) buffer_.chop(1);
        QJsonObject object;
        if (!decodeMessage(buffer_, &object, error)) {
            failed_ = true;
            buffer_.clear();
            return false;
        }
        messages->append(object);
        buffer_.clear();
        offset = end + 1;
    }
    return true;
}

bool ProtocolDecoder::finish(QString *error) const
{
    if (!failed_ && buffer_.isEmpty()) return true;
    *error = QStringLiteral("Engine closed with an incomplete protocol response.");
    return false;
}

void ProtocolDecoder::reset()
{
    buffer_.clear();
    failed_ = false;
}

} // namespace guard
