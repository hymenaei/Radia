/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include <string>
#include <string_view>
#include "diagnostic.h"
#include "localizedtext.h"

namespace radia::ui {
class Document;
class Element;
class System;
class SettingResolver;
struct EventRegistrationDescriptor;
} // namespace radia::ui

namespace radia::viewer::ui {
using radia::ui::DiagnosticResult;
using radia::ui::Document;
using radia::ui::Element;
using radia::ui::EventRegistrationDescriptor;
using radia::ui::LocalizationArguments;
using radia::ui::LocalizedText;
using radia::ui::SettingResolver;
using radia::ui::System;

class ComponentManager;

class DocumentController {
public:
    DocumentController(System& system, Document& document);
    virtual ~DocumentController();

    virtual void onOpen() {}
    virtual void onClose() {}
    virtual void onReloadSucceeded() {}
    virtual void onReloadFailed(const DiagnosticResult&) {}

protected:
    System& system() noexcept { return mSystem; }
    const System& system() const noexcept { return mSystem; }
    Element* getElementById(std::string_view id);
    LocalizedText t(std::string localizationKey, LocalizationArguments arguments = {}) const;

    template<typename Callback> void handler(std::string handlerName, Callback callback);
    template<typename ControllerT, typename... Args> void handler(std::string handlerName, void (ControllerT::*method)(Args...));
    template<typename ControllerT, typename... Args> void handler(std::string handlerName, void (ControllerT::*method)(Args...) const);

private:
    class PreparedMount;
    struct PreparedMountResult;

    PreparedMountResult prepare(SettingResolver& settingResolver);
    bool canCommit(const PreparedMount& prepared) const;
    bool commit(PreparedMount&& prepared);
    void deactivate() noexcept;
    bool activate();
    void addHandlerRegistration(EventRegistrationDescriptor registration);

    System& mSystem;
    Document& mDocument;
    struct Impl;
    std::unique_ptr<Impl> mImpl;

    friend class ComponentManager;
};
} // namespace radia::viewer::ui
