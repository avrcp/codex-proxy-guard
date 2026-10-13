#pragma once

#include "core.h"

namespace cpg::detail {

// Pure protocol validators; exposed privately for side-effect-free fixtures.
Json activationRequest(const std::string &bytes);
Json activationReceipt(const std::string &bytes);
std::string classifyInstance(std::uint64_t creation, std::uint64_t submission);
void observeImage(Json &receipt, const std::wstring &actual, const std::wstring &expected);

} // namespace cpg::detail
