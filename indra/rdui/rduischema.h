#ifndef LL_RDUI_SCHEMA_H
#define LL_RDUI_SCHEMA_H

#include <string>
#include <string_view>

namespace rdui
{
    std::string schemaNameKey(std::string_view name);
    bool isLocalIdentifier(std::string_view value);
}

#endif // LL_RDUI_SCHEMA_H
