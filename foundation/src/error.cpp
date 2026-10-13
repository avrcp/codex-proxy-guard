#include "cpg/error.h"
#include "cpg/text.h"

namespace cpg {

Error::Error(std::string errorCode, std::string text, std::string errorField)
    : std::runtime_error(errorCode), code(std::move(errorCode)),
      message(publicText(text, 4096)), field(std::move(errorField))
{
}

} // namespace cpg
