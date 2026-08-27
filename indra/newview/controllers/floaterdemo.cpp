/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "floaterdemo.h"
#include <algorithm>
#include <iterator>
#include <tuple>
#include <utility>
#include "controllerregistration.h"
#include "runtime.h"
#include "system.h"

namespace radia::viewer::ui {
using radia::ui::Diagnostic;
using radia::ui::DiagnosticResult;
using radia::ui::Document;
using radia::ui::Event;
using radia::ui::LocaleInfo;
using radia::ui::System;

void registerFloaterDemo(Runtime& runtime) {
    runtime.registerFloater("floaterDemo", "floater_demo.html", [&runtime](System& system, Document& document) {
        return std::make_unique<FloaterDemo>(system, document, [&runtime] { runtime.requestSkinReload(); });
    });
}

FloaterDemo::FloaterDemo(System& system, Document& document, std::function<void()> requestSkinReload)
    : DocumentController(system, document), mRequestSkinReload(std::move(requestSkinReload)) {
    mStatus = getElementById("status");
    mActiveLocale = getElementById("activeLocale");
    mPreviousLocale = getElementById("previousLocale");
    mNextLocale = getElementById("nextLocale");
    handler("press", &FloaterDemo::press);
    handler("switchChanged", &FloaterDemo::switchChanged);
    handler("selectLocale", &FloaterDemo::selectLocale);
    handler("requestSkinReload", &FloaterDemo::requestSkinReload);
}

void FloaterDemo::refreshLocaleControls() {
    const LocaleInfo* active = system().activeLocaleInfo();
    if (mActiveLocale) mActiveLocale->textContent(active ? active->name : std::string());
    const bool disabled = system().locales().size() <= 1;
    if (mPreviousLocale) mPreviousLocale->disabled(disabled);
    if (mNextLocale) mNextLocale->disabled(disabled);
}

void FloaterDemo::press() {
    if (mStatus) mStatus->content(t("demo.clicked"));
}

void FloaterDemo::switchChanged(const Event& event) {
    if (mStatus) mStatus->content(t(event.checked() ? "demo.switchOn" : "demo.switchOff"));
}

void FloaterDemo::selectLocale(int step) {
    std::vector<LocaleInfo> locales = system().locales();
    std::sort(locales.begin(), locales.end(), [](const LocaleInfo& left, const LocaleInfo& right) {
        return std::tie(left.name, left.localeId) < std::tie(right.name, right.localeId);
    });
    if (locales.size() <= 1 || step == 0) return;
    const auto current =
        std::find_if(locales.begin(), locales.end(), [this](const LocaleInfo& locale) { return locale.localeId == system().activeLocale(); });
    const std::size_t index = current == locales.end() ? 0 : static_cast<std::size_t>(std::distance(locales.begin(), current));
    const std::size_t next = step < 0 ? (index + locales.size() - 1) % locales.size() : (index + 1) % locales.size();
    if (!system().setLocale(locales[next].localeId)) return;
    refreshLocaleControls();
}

void FloaterDemo::requestSkinReload() {
    if (mRequestSkinReload) mRequestSkinReload();
}

void FloaterDemo::onReloadSucceeded() {
    if (mStatus) mStatus->content(t("demo.reloadSucceeded"));
}

void FloaterDemo::onReloadFailed(const DiagnosticResult& diagnostics) {
    std::string message = system().resolveText("demo.reloadFailed");
    if (!diagnostics.errors.empty()) {
        const Diagnostic& error = diagnostics.errors.front();
        message += ": " + error.code + ": " + error.formatted();
        if (diagnostics.errors.size() > 1) message += " (+" + std::to_string(diagnostics.errors.size() - 1) + ")";
    }
    if (mStatus) mStatus->textContent(std::move(message));
}
} // namespace radia::viewer::ui
