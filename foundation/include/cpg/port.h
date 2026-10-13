#pragma once
#include "error.h"
#include <string_view>

namespace cpg {
inline int parsePortStrict(std::string_view text)
{
    if (text.empty() || text.size() > 5)
        throw Error("CONFIG_INVALID", "Proxy port must be an integer from 1 to 65535.", "proxy.port");
    unsigned value = 0;
    for (const char digit : text) {
        if (digit < '0' || digit > '9')
            throw Error("CONFIG_INVALID", "Proxy port must contain only ASCII digits.", "proxy.port");
        value = value * 10 + static_cast<unsigned>(digit - '0');
    }
    if (value == 0 || value > 65535)
        throw Error("CONFIG_INVALID", "Proxy port must be an integer from 1 to 65535.", "proxy.port");
    return static_cast<int>(value);
}
}
