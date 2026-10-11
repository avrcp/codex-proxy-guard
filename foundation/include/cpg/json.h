#pragma once

#ifdef _MSC_VER
#pragma warning(push, 0)
#endif
#include "json.hpp"
#ifdef _MSC_VER
#pragma warning(pop)
#endif

#include <cstddef>
#include <string>
#include <string_view>

namespace cpg {

using Json = nlohmann::json;

// Strict JSON object parser used by every protocol boundary. Enforces, in
// order: a size limit; strict UTF-8 (validated independently of the parser so
// the guarantee holds regardless of parser internals); rejection of a UTF-8
// BOM prefix, comments, trailing garbage, nesting beyond 64 levels and
// duplicate object keys (compared after escape decoding) at every nesting
// level; and a JSON object at the root. Any violation throws Error(code, ...)
// — parsing never partially succeeds.
Json parseStrictJsonObject(std::string_view bytes, std::size_t limit,
                           std::string_view errorCode);

// Compact single-line serialization (UTF-8, non-ASCII kept verbatim).
std::string jsonDump(const Json &value);

} // namespace cpg
