/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <optional>
#include "binding/binder.h"
#include "dom/document.h"
#include "dom/elementinternal.h"
#include "dom/text.h"
#include "html/element.h"
#include "html/elementnames.h"
#include "html/floater.h"
#include "layout/engine.h"
#include "layout/primitives.h"
#include "paint/paintcontext.h"
#include "style/stylepass.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"

namespace radia::ui {
using detail::ElementInternalAccess;
using detail::MountEpoch;
using detail::NodeRef;

namespace {
bool isDetachedOwnedRoot(const Element& element) {
    return !element.parentNode() && !ElementInternalAccess::isMounted(element);
}

bool isDetachedBorrowedRoot(const Element& element) {
    return !element.parentElement() && (!element.parentNode() || element.parentNode()->asDocument()) && !ElementInternalAccess::isMounted(element);
}
} // namespace

Surface::ElementObservation Surface::observe(Element& element) const {
    return ElementObservation(element);
}
Surface::ConstElementObservation Surface::observe(const Element& element) const {
    return ConstElementObservation(element);
}

Surface::Surface() : mTextMetrics(fixedTextMetrics()), mObservedTextMetricsGeneration(mTextMetrics.generation()) {}

Surface::Surface(const StyleSheet& styleSheet)
    : mStyleSheet(&styleSheet), mTextMetrics(fixedTextMetrics()), mObservedStyleGeneration(styleSheet.generation()),
      mObservedTextMetricsGeneration(mTextMetrics.generation()) {}

Surface::Surface(const System& system, const TextMetrics& textMetrics)
    : mStyleSheet(&system.styleSheet()), mSystem(&system), mTextMetrics(textMetrics), mObservedStyleGeneration(system.generation()),
      mObservedTextMetricsGeneration(mTextMetrics.generation()) {
    mScrollLayoutOptions.nativeMetrics = system.nativeAppearance().layoutMetrics();
    mNativeAppearanceRevision = mScrollLayoutOptions.nativeMetrics.revision;
    system.registerSurface(*this);
}

Surface::Mount::Mount(std::unique_ptr<Element> root, SurfaceLayer layer, HTMLFloaterElement* floater)
    : root(root.get()), ownedRoot(std::move(root)), layer(layer), ownership(Ownership::Owned), floater(floater) {}

Surface::Mount::Mount(Element& root, SurfaceLayer layer, HTMLFloaterElement* floater)
    : root(&root), layer(layer), ownership(Ownership::Borrowed), floater(floater) {}

Surface::Mount::~Mount() = default;

Surface::~Surface() {
    mLifetime.reset();

    std::vector<ElementRef<Element>> mountedRoots;
    for (MountList& layerMounts : mMounts)
        for (MountPtr& mount : layerMounts) {
            if (!mount) continue;
            detachBindings(*mount);
            if (!mount->root) continue;
            mountedRoots.emplace_back(mount->root);
            mount->lifetime.reset();
            mount->root = nullptr;
            mount->floater = nullptr;
        }

    clearInteractionState();
    for (const ElementRef<Element>& rootRef : mountedRoots)
        if (Element* root = rootRef.get(); root && root->mSurface == this) root->setSurface(nullptr);

    for (MountList& layerMounts : mMounts) layerMounts.clear();
    if (mSystem) mSystem->unregisterSurface(*this);
}

Element& Surface::mount(std::unique_ptr<Element> element, SurfaceLayer layer) {
    llassert_always(element);
    HTMLFloaterElement* floater = dynamic_cast<HTMLFloaterElement*>(element.get());
    return installMount(std::make_unique<Mount>(std::move(element), layer, floater));
}

Element& Surface::mount(Document& document, SurfaceLayer layer) {
    Element* root = document.documentElement();
    llassert_always(root && root->parentNode() == &document && !root->surface());

    MountPtr mount = std::make_unique<Mount>(*root, layer, dynamic_cast<HTMLFloaterElement*>(root));
    const std::weak_ptr<char> surfaceLifetime = mLifetime;
    const std::weak_ptr<char> rootLifetime = ElementInternalAccess::lifetime(*root);
    const std::weak_ptr<char> mountLifetime = mount->lifetime;
    document.addDestructionObserver([this, root, surfaceLifetime, rootLifetime, mountLifetime] {
        if (surfaceLifetime.expired() || rootLifetime.expired() || mountLifetime.expired()) return;
        (void)unmountBorrowed(*root);
    });

    return installMount(std::move(mount));
}

Element& Surface::mount(Element& element, SurfaceLayer layer) {
    return installMount(std::make_unique<Mount>(element, layer, dynamic_cast<HTMLFloaterElement*>(&element)));
}

Element& Surface::installMount(MountPtr mount) {
    llassert_always(mount
                    && mount->root
                    && (mount->ownership == Mount::Ownership::Owned ? isDetachedOwnedRoot(*mount->root) : isDetachedBorrowedRoot(*mount->root)));
    if (mount->layer == SurfaceLayer::Modal) clearInteractionState();

    Element* mounted = mount->root;
    ElementRef<Element> mountedRef(mounted);
    mounts(mount->layer).emplace_back(std::move(mount));
    mounted->setSurface(this);
    Element* current = mountedRef.get();
    llassert_always(current && current->parentElement() == nullptr && current->surface() == this);
    invalidateOrderingCache();
    requestLayout();
    return *current;
}

Surface::MountList& Surface::mounts(SurfaceLayer layer) {
    return mMounts[static_cast<std::size_t>(layer)];
}

const Surface::MountList& Surface::mounts(SurfaceLayer layer) const {
    return mMounts[static_cast<std::size_t>(layer)];
}

Surface::Mount* Surface::findMount(Element* element) noexcept {
    if (!element) return nullptr;
    for (MountList& layerMounts : mMounts)
        for (MountPtr& mount : layerMounts)
            if (mount && mount->root == element) return mount.get();
    return nullptr;
}

bool Surface::attachBinding(Element& root, Binding& binding) {
    Element* mounted = mountedRoot(&root);
    Mount* mount = findMount(mounted);
    if (!mount) return false;
    if (std::find(mount->bindings.begin(), mount->bindings.end(), &binding) == mount->bindings.end()) mount->bindings.push_back(&binding);
    binding.mAttachedSurface = this;
    return true;
}

void Surface::detachBinding(Binding& binding) noexcept {
    for (MountList& layerMounts : mMounts)
        for (MountPtr& mount : layerMounts)
            if (mount) mount->bindings.erase(std::remove(mount->bindings.begin(), mount->bindings.end(), &binding), mount->bindings.end());
    if (binding.mAttachedSurface == this) binding.mAttachedSurface = nullptr;
}

void Surface::replaceBinding(Binding& current, Binding& replacement) noexcept {
    bool replaced = false;
    for (MountList& layerMounts : mMounts) {
        for (MountPtr& mount : layerMounts) {
            if (!mount) continue;
            for (Binding*& binding : mount->bindings) {
                if (binding != &current) continue;
                binding = &replacement;
                replaced = true;
            }
        }
    }
    current.mAttachedSurface = nullptr;
    replacement.mAttachedSurface = replaced ? this : nullptr;
}

void Surface::detachBindings(Mount& mount) noexcept {
    std::vector<Binding*> bindings;
    bindings.swap(mount.bindings);
    for (Binding* binding : bindings) {
        if (!binding || binding->mAttachedSurface != this) continue;
        binding->mAttachedSurface = nullptr;
        binding->deactivate();
    }
}

const Surface::Mount* Surface::findMount(const Element* element) const noexcept {
    if (!element) return nullptr;
    for (const MountList& layerMounts : mMounts)
        for (const MountPtr& mount : layerMounts)
            if (mount && mount->root == element) return mount.get();
    return nullptr;
}

Surface::MountPtr Surface::detachMount(Element& element) {
    for (MountList& layerMounts : mMounts) {
        const auto found =
            std::find_if(layerMounts.begin(), layerMounts.end(), [&element](const MountPtr& mount) { return mount && mount->root == &element; });
        if (found == layerMounts.end()) continue;

        MountPtr detached = std::move(*found);
        layerMounts.erase(found);
        detachBindings(*detached);
        ElementRef<Element> rootRef(detached->root);
        detached->lifetime.reset();
        detached->root = nullptr;
        detached->floater = nullptr;
        clearInteractionState();
        if (Element* root = rootRef.get(); root && root->surface() == this) root->setSurface(nullptr);
        invalidateOrderingCache();
        requestLayout();
        refreshHover();
        return detached;
    }
    return nullptr;
}

void Surface::elementOwnerDestroyed(Element& element) {
    const Mount* mount = findMount(&element);
    if (!mount || mount->ownership != Mount::Ownership::Borrowed) return;
    (void)detachMount(element);
}

std::unique_ptr<Element> Surface::unmount(Element& element) {
    const Mount* mount = findMount(&element);
    if (!mount || mount->ownership != Mount::Ownership::Owned) return nullptr;
    MountPtr detached = detachMount(element);
    if (!detached) return nullptr;
    return std::move(detached->ownedRoot);
}

bool Surface::unmountBorrowed(Element& element) {
    const Mount* mount = findMount(&element);
    if (!mount || mount->ownership != Mount::Ownership::Borrowed) return false;
    return static_cast<bool>(detachMount(element));
}

void Surface::clearLayer(SurfaceLayer layer) {
    MountList& layerMounts = mounts(layer);
    while (!layerMounts.empty()) {
        Element* root = layerMounts.front() ? layerMounts.front()->root : nullptr;
        if (!root) {
            layerMounts.erase(layerMounts.begin());
            continue;
        }
        (void)detachMount(*root);
    }
    invalidateOrderingCache();
    requestLayout();
    refreshHover();
}

Element* Surface::mountedRoot(Element* element) {
    if (!element) return nullptr;
    Element* root = element;
    while (root->parentElement()) root = root->parentElement();
    return isSurfaceRoot(root) ? root : nullptr;
}

const Element* Surface::mountedRoot(const Element* element) const {
    if (!element) return nullptr;
    const Element* root = element;
    while (root->parentElement()) root = root->parentElement();
    return isSurfaceRoot(root) ? root : nullptr;
}

std::optional<SurfaceLayer> Surface::layerOf(const Element* element) const {
    const Element* root = mountedRoot(element);
    if (!root) return std::nullopt;
    const Mount* mount = findMount(root);
    return mount ? std::optional<SurfaceLayer>(mount->layer) : std::nullopt;
}

bool Surface::isSurfaceRoot(const Element* element) const {
    return element && !element->parentElement() && findMount(element);
}

void Surface::localeChanged() {
    if (!mSystem) return;
    const auto refresh = [this](auto&& self, Element& element) -> void {
        const ElementRef<Element> lifetime(&element);
        const Surface* surface = element.mSurface;
        const Element* parent = element.mParent;
        std::vector<ElementRef<Element>> children;
        const ElementList authoredChildren = element.children();
        children.reserve(authoredChildren.size());
        for (Element* child : authoredChildren) children.emplace_back(child);
        element.onLocaleChanged(*mSystem);
        Element* current = lifetime.get();
        if (!current || current->mSurface != surface || current->mParent != parent) return;
        for (const ElementRef<Element>& childRef : children)
            if (Element* child = childRef.get(); child && child->parentElement() == current) self(self, *child);
    };
    std::vector<ElementRef<Element>> mountedRoots;
    for (const MountList& layerMounts : mMounts)
        for (const MountPtr& mount : layerMounts)
            if (mount && mount->root) mountedRoots.emplace_back(mount->root);
    for (const ElementRef<Element>& rootRef : mountedRoots)
        if (Element* root = rootRef.get(); root && root->mSurface == this && isSurfaceRoot(root)) refresh(refresh, *root);
    requestLayout();
}

void Surface::keybindingsChanged() {
    if (!mSystem) return;
    const auto refresh = [this](auto&& self, Element& element) -> void {
        const ElementRef<Element> lifetime(&element);
        const Surface* surface = element.mSurface;
        const Element* parent = element.mParent;
        std::vector<ElementRef<Element>> children;
        const ElementList authoredChildren = element.children();
        children.reserve(authoredChildren.size());
        for (Element* child : authoredChildren) children.emplace_back(child);
        if (auto* htmlElement = dynamic_cast<HTMLElement*>(&element)) htmlElement->onKeybindingsChanged(*mSystem);
        Element* current = lifetime.get();
        if (!current || current->mSurface != surface || current->mParent != parent) return;
        for (const ElementRef<Element>& childRef : children)
            if (Element* child = childRef.get(); child && child->parentElement() == current) self(self, *child);
    };
    std::vector<ElementRef<Element>> mountedRoots;
    for (const MountList& layerMounts : mMounts)
        for (const MountPtr& mount : layerMounts)
            if (mount && mount->root) mountedRoots.emplace_back(mount->root);
    for (const ElementRef<Element>& rootRef : mountedRoots)
        if (Element* root = rootRef.get(); root && root->mSurface == this && isSurfaceRoot(root)) refresh(refresh, *root);
}
} // namespace radia::ui
