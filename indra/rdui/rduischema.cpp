#include "linden_common.h"
#include "rduischema.h"

namespace rdui
{
    std::string schemaNameKey(std::string_view name)
    {
        std::string key;
        key.reserve(name.size());
        for (char character : name)
        {
            if (character >= 'A' && character <= 'Z') character = static_cast<char>(character - 'A' + 'a');
            key.push_back(character);
        }
        return key;
    }

    bool isLocalIdentifier(std::string_view value)
    {
        if (value.empty() || value.front() == '-' || value.back() == '-') return false;
        bool previous_hyphen = false;
        for (char character : value)
        {
            if (character == '-')
            {
                if (previous_hyphen) return false;
                previous_hyphen = true;
                continue;
            }
            previous_hyphen = false;
            if ((character < 'a' || character > 'z') && (character < '0' || character > '9')) return false;
        }
        return true;
    }
}
