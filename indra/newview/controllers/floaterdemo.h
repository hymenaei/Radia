/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <functional>
#include "documentcontroller.h"
#include "event.h"

namespace radia::viewer::ui {
using radia::ui::Event;

class Runtime;

class FloaterDemo final : public DocumentController {
public:
    FloaterDemo(System& system, Document& document, std::function<void()> requestSkinReload = {});

    void refreshLocaleControls();
    void onReloadSucceeded() override;
    void onReloadFailed(const DiagnosticResult& diagnostics) override;

private:
    void press();
    void switchChanged(const Event& event);
    void selectLocale(int step);
    void requestSkinReload();

    Element* mStatus = nullptr;
    Element* mActiveLocale = nullptr;
    Element* mPreviousLocale = nullptr;
    Element* mNextLocale = nullptr;
    std::function<void()> mRequestSkinReload;
};

void registerFloaterDemo(Runtime& runtime);
} // namespace radia::viewer::ui
