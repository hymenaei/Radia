/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <Core/Diagnostic.h>
#include <Core/LocalizedText.h>
#include <memory>
#include <string>
#include <string_view>

namespace Core {
class Document;
class Element;
class System;
class SettingResolver;
struct EventRegistrationDescriptor;
} // namespace Core

namespace Viewer {
class ComponentManager;

class DocumentController {
public:
    DocumentController(Core::System& system, Core::Document& document);
    virtual ~DocumentController();

    virtual void onOpen() {}
    virtual void onClose() {}
    virtual void onReloadSucceeded() {}
    virtual void onReloadFailed(const Core::DiagnosticResult&) {}

protected:
    Core::System& system() noexcept { return mSystem; }
    const Core::System& system() const noexcept { return mSystem; }
    Core::Element* getElementById(std::string_view id);
    Core::LocalizedText t(std::string localizationKey, Core::LocalizationArguments arguments = {}) const;

    template<typename Callback> void handler(std::string handlerName, Callback callback);
    template<typename ControllerT, typename... Args> void handler(std::string handlerName, void (ControllerT::*method)(Args...));
    template<typename ControllerT, typename... Args> void handler(std::string handlerName, void (ControllerT::*method)(Args...) const);

private:
    class PreparedMount;
    struct PreparedMountResult;

    PreparedMountResult prepare(Core::SettingResolver& settingResolver);
    bool canCommit(const PreparedMount& prepared) const;
    bool commit(PreparedMount&& prepared);
    void deactivate() noexcept;
    bool activate();
    void addHandlerRegistration(Core::EventRegistrationDescriptor registration);

    Core::System& mSystem;
    Core::Document& mDocument;
    struct Impl;
    std::unique_ptr<Impl> mImpl;

    friend class ComponentManager;
};
} // namespace Viewer
