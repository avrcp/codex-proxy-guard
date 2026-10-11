#pragma once

#include "core.h"

namespace cpg::detail {

// Pure protocol validators; exposed privately for side-effect-free fixtures.
Json activationRequest(const std::string &bytes);
Json activationReceipt(const std::string &bytes);

} // namespace cpg::detail
