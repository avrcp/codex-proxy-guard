#include "cpg/json.h"
#include "cpg/error.h"
#include "cpg/text.h"

#include <unordered_set>
#include <vector>

namespace cpg {
namespace {

constexpr int maxDepth = 64;

struct KeyWalker {
    bool duplicateKey = false;
    bool overDepth = false;
    std::vector<std::unordered_set<std::string>> objectKeys;

    bool operator()(int depth, Json::parse_event_t event, Json &parsed)
    {
        if (depth > maxDepth) overDepth = true;
        switch (event) {
        case Json::parse_event_t::object_start:
            objectKeys.emplace_back();
            break;
        case Json::parse_event_t::key:
            if (!objectKeys.empty()) {
                // nlohmann hands the already escape-decoded key to the
                // callback before it is lost, so duplicate detection compares
                // decoded spellings ("a" vs "\u0061") at every level.
                try {
                    if (!objectKeys.back().insert(parsed.get<std::string>()).second)
                        duplicateKey = true;
                } catch (const Json::exception &) {
                    duplicateKey = true;
                }
            }
            break;
        case Json::parse_event_t::object_end:
            if (!objectKeys.empty()) objectKeys.pop_back();
            break;
        default:
            break;
        }
        return true;
    }
};

} // namespace

Json parseStrictJsonObject(std::string_view bytes, std::size_t limit, std::string_view errorCode)
{
    if (bytes.size() > limit || !strictUtf8(bytes))
        throw Error(std::string(errorCode), "JSON input is oversized or not valid UTF-8.");
    std::string_view payload = bytes;
    if (payload.substr(0, 3) == "\xef\xbb\xbf") payload = payload.substr(3);
    KeyWalker walker;
    Json parsed;
    try {
        // The callback functor is copied into the parser; capture the walker
        // by reference so its flags survive into the checks below.
        auto callback = [&walker](int depth, Json::parse_event_t event, Json &value) {
            return walker(depth, event, value);
        };
        parsed = Json::parse(payload.begin(), payload.end(), callback,
                             /*allow_exceptions=*/true, /*ignore_comments=*/false);
    } catch (const Json::exception &) {
        throw Error(std::string(errorCode), "Expected a valid JSON object.");
    }
    if (walker.overDepth) throw Error(std::string(errorCode), "JSON nesting exceeds its limit.");
    if (walker.duplicateKey) throw Error(std::string(errorCode), "Duplicate JSON object key.");
    if (!parsed.is_object()) throw Error(std::string(errorCode), "Expected a valid JSON object.");
    return parsed;
}

std::string jsonDump(const Json &value)
{
    return value.dump();
}

} // namespace cpg
