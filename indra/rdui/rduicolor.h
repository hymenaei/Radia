#ifndef LL_RDUI_COLOR_H
#define LL_RDUI_COLOR_H

#include "rduitypes.h"
#include <optional>
#include <string>

namespace rdui
{
    std::optional<Color> parseColor(const std::string& value);
    bool isColorSyntax(const std::string& value);
}

#endif // LL_RDUI_COLOR_H
