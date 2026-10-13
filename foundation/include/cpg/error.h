#pragma once

#include "json.h"

#include <stdexcept>
#include <string>
#include <utility>

namespace cpg {

// Public error type. The message is passed through publicText() at
// construction: bounded length, redacted and stripped of control characters
// before it can reach any display or protocol surface.
struct Error final : std::runtime_error {
    std::string code;
    std::string message;
    std::string field;

    Error(std::string errorCode, std::string text, std::string errorField = {});
    Json json() const {
        Json value{{"code", code}, {"message", message}};
        if (!field.empty()) value["field"] = field;
        return value;
    }
};

} // namespace cpg
