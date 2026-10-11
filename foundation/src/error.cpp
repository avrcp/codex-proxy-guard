#include "cpg/error.h"
#include "cpg/text.h"

namespace cpg {

Error::Error(std::string errorCode, std::string text)
    : std::runtime_error(errorCode), code(std::move(errorCode)),
      message(publicText(text, 1024))
{
}

} // namespace cpg
