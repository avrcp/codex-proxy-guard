#include "cpg/build_info.h"

#include <string_view>

#ifndef CPG_PRODUCT_VERSION
#define CPG_PRODUCT_VERSION "unknown"
#endif
#ifndef CPG_BUILD_COMMIT
#define CPG_BUILD_COMMIT "unknown"
#endif
#ifndef CPG_BUILD_DIRTY
#define CPG_BUILD_DIRTY "true"
#endif

namespace cpg {

const char *productVersion() { return CPG_PRODUCT_VERSION; }
const char *buildCommit() { return CPG_BUILD_COMMIT; }
bool buildDirty() { return "true" == std::string_view(CPG_BUILD_DIRTY); }

} // namespace cpg
