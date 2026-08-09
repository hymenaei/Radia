/**
 * @file floaterstatestore_test.cpp
 * @brief Tests persistence of open Floater documents and placement state.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
 */

#include "linden_common.h"
#include "../test/lltut.h"
#include "floaterstatestore.h"
#include "llcontrol.h"

LLControlGroup gSavedSettings("test");

namespace tut {
struct floater_state_store {
    floater_state_store() {
        if (!gSavedSettings.controlExists("RduiOpenFloaters"))
            gSavedSettings.declareLLSD("RduiOpenFloaters", LLSD::emptyArray(), "", LLControlVariable::PERSIST_NO);
        if (!gSavedSettings.controlExists("RduiFloaterPlacements"))
            gSavedSettings.declareLLSD("RduiFloaterPlacements", LLSD::emptyMap(), "", LLControlVariable::PERSIST_NO);
        gSavedSettings.setLLSD("RduiOpenFloaters", LLSD::emptyArray());
        gSavedSettings.setLLSD("RduiFloaterPlacements", LLSD::emptyMap());
    }
};

using floater_state_store_group = test_group<floater_state_store>;
using floater_state_store_object = floater_state_store_group::object;
floater_state_store_group floater_state_store_tests("RduiFloaterStateStore");

template<> template<> void floater_state_store_object::test<1>() {
    set_test_name("open Floater documents use the stable ordered LLSD shape");
    rdui::viewer::FloaterStateStore store;
    const std::vector<rdui::viewer::FloaterDocumentId> documents{{"inventory", "one"}, {"inventory", "two"}, {"settings", ""}};

    store.saveOpenDocuments(documents);

    const LLSD saved = gSavedSettings.getLLSD("RduiOpenFloaters");
    ensure("open documents are stored as an array", saved.isArray());
    ensure_equals("open document order and count are retained", saved.size(), std::size_t{3});
    for (std::size_t index = 0; index < saved.size(); ++index) {
        const LLSD& entry = saved[static_cast<LLSD::Integer>(index)];
        ensure("open document entry is a map", entry.isMap());
        ensure_equals("open document entry has only contract fields", entry.size(), std::size_t{2});
        ensure("definition is stored as a string", entry["definition"].isString());
        ensure("instance is stored as a string", entry["instance"].isString());
        ensure_equals("definition order is retained", entry["definition"].asString(), documents[index].definitionId);
        ensure_equals("instance order is retained", entry["instance"].asString(), documents[index].instanceKey);
    }

    ensure("saved documents round trip", store.openDocuments() == documents);
}

template<> template<> void floater_state_store_object::test<2>() {
    set_test_name("malformed open Floater entries are ignored and empty instances remain valid");
    rdui::viewer::FloaterStateStore store;
    gSavedSettings.setLLSD("RduiOpenFloaters", LLSD::emptyMap());
    ensure("non-array open document state is ignored", store.openDocuments().empty());

    LLSD saved = LLSD::emptyArray();
    saved.append("not-a-map");
    saved.append(LLSD::emptyMap());
    LLSD empty_definition = LLSD::emptyMap();
    empty_definition["definition"] = "";
    empty_definition["instance"] = "ignored";
    saved.append(empty_definition);
    LLSD singleton = LLSD::emptyMap();
    singleton["definition"] = "singleton";
    saved.append(singleton);
    LLSD keyed = LLSD::emptyMap();
    keyed["definition"] = "inventory";
    keyed["instance"] = "second";
    saved.append(keyed);
    gSavedSettings.setLLSD("RduiOpenFloaters", saved);

    const std::vector<rdui::viewer::FloaterDocumentId> restored = store.openDocuments();
    ensure_equals("only valid definitions are restored", restored.size(), std::size_t{2});
    ensure_equals("missing instance becomes the valid empty instance key", restored[0].definitionId, std::string("singleton"));
    ensure("empty instance key is retained", restored[0].instanceKey.empty());
    ensure("valid keyed document is retained", restored[1] == rdui::viewer::FloaterDocumentId{"inventory", "second"});
}

template<> template<> void floater_state_store_object::test<3>() {
    set_test_name("placement persistence uses only the stable saved setting");
    rdui::viewer::FloaterStateStore store;
    LLSD open_documents = LLSD::emptyArray();
    open_documents.append("sentinel");
    gSavedSettings.setLLSD("RduiOpenFloaters", open_documents);

    LLSD initial = LLSD::emptyMap();
    initial["inventory"]["x"] = 12.f;
    gSavedSettings.setLLSD("RduiFloaterPlacements", initial);
    rdui::viewer::FloaterPlacementStore::Persistence& persistence = store.placementPersistence();
    const LLSD restored_placements = persistence.read();
    ensure("restored placements are a one-entry map", restored_placements.isMap() && restored_placements.size() == 1);
    ensure("restored placement is a one-entry map", restored_placements["inventory"].isMap() && restored_placements["inventory"].size() == 1);
    ensure("restored placement field remains real", restored_placements["inventory"]["x"].isReal());
    ensure_equals("restored placement value", restored_placements["inventory"]["x"].asReal(), 12.0);

    LLSD replacement = LLSD::emptyMap();
    replacement["settings"]["detached"] = false;
    persistence.write(replacement);
    const LLSD saved_placements = gSavedSettings.getLLSD("RduiFloaterPlacements");
    ensure("saved placements are a one-entry map", saved_placements.isMap() && saved_placements.size() == 1);
    ensure("saved placement is a one-entry map", saved_placements["settings"].isMap() && saved_placements["settings"].size() == 1);
    ensure("saved detached field remains Boolean", saved_placements["settings"]["detached"].isBoolean());
    ensure_equals("saved detached value", saved_placements["settings"]["detached"].asBoolean(), false);

    const LLSD preserved_documents = gSavedSettings.getLLSD("RduiOpenFloaters");
    ensure("placement writes preserve the document array", preserved_documents.isArray() && preserved_documents.size() == 1);
    ensure("placement writes preserve the document type", preserved_documents[0].isString());
    ensure_equals("placement writes preserve the document value", preserved_documents[0].asString(), std::string("sentinel"));
}
} // namespace tut
