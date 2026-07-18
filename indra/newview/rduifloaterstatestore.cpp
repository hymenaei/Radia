#include "llviewerprecompiledheaders.h"
#include "rduifloaterstatestore.h"

#include "llviewercontrol.h"

#include <utility>

namespace rdui::viewer
{
    std::vector<FloaterDocumentId> FloaterStateStore::openDocuments() const
    {
        std::vector<FloaterDocumentId> result;
        const LLSD saved = gSavedSettings.getLLSD("RduiOpenFloaters");
        if (!saved.isArray())
        {
            LL_WARNS("rdui") << "RduiOpenFloaters is not an array; ignoring it." << LL_ENDL;
            return result;
        }

        for (LLSD::array_const_iterator entry = saved.beginArray(); entry != saved.endArray(); ++entry)
        {
            if (!entry->isMap()) continue;
            const std::string definition_id = (*entry)["definition"].asString();
            if (definition_id.empty()) continue;
            result.push_back({definition_id, (*entry)["instance"].asString()});
        }
        return result;
    }

    void FloaterStateStore::saveOpenDocuments(const std::vector<FloaterDocumentId>& documents)
    {
        LLSD saved = LLSD::emptyArray();
        for (const FloaterDocumentId& document : documents)
        {
            LLSD entry = LLSD::emptyMap();
            entry["definition"] = document.definitionId;
            entry["instance"] = document.instanceKey;
            saved.append(std::move(entry));
        }
        gSavedSettings.setLLSD("RduiOpenFloaters", saved);
    }

    LLSD FloaterStateStore::read() const
    {
        return gSavedSettings.getLLSD("RduiFloaterPlacements");
    }

    void FloaterStateStore::write(const LLSD& placements)
    {
        gSavedSettings.setLLSD("RduiFloaterPlacements", placements);
    }
}
