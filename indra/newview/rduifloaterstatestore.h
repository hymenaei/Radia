#ifndef LL_RDUI_FLOATER_STATE_STORE_H
#define LL_RDUI_FLOATER_STATE_STORE_H

#include "rduifloaterdocumentmanager.h"
#include "rduifloaterplacementstore.h"

#include <vector>

namespace rdui::viewer
{
    class FloaterStateStore final : private FloaterPlacementStore::Persistence
    {
        public:
            std::vector<FloaterDocumentId> openDocuments() const;
            void saveOpenDocuments(const std::vector<FloaterDocumentId>& documents);

            FloaterPlacementStore::Persistence& placementPersistence() { return *this; }

        private:
            LLSD read() const override;
            void write(const LLSD& placements) override;
    };
}

#endif // LL_RDUI_FLOATER_STATE_STORE_H
