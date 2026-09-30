/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <Core/Event.h>
#include <functional>
#include "DocumentController.h"

namespace Viewer {
class Runtime;

class FloaterDemo final : public DocumentController {
public:
    FloaterDemo(Core::System& system, Core::Document* document, std::function<void()> requestSkinReload = {});

    void refreshLocaleControls();
    void onOpen() override;
    void onReloadSucceeded() override;
    void onReloadFailed(const Core::DiagnosticResult& diagnostics) override;

private:
    void press();
    void switchChanged(const Core::Event& event);
    void selectLocale(int step);
    void requestSkinReload();

    Core::Element* mStatus = nullptr;
    Core::Element* mActiveLocale = nullptr;
    Core::Element* mPreviousLocale = nullptr;
    Core::Element* mNextLocale = nullptr;
    std::function<void()> mRequestSkinReload;
};

void registerFloaterDemo(Runtime& runtime);
} // namespace Viewer
