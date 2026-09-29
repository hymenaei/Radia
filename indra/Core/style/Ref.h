/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <atomic>
#include <utility>

namespace Core::Style {
template<typename T> class Ref;

template<typename T> Ref<T> adoptRef(T&);

template<typename T> class RefCounted {
public:
    RefCounted() = default;
    RefCounted(const RefCounted&) noexcept
        : mRefCount(1) {}
    RefCounted& operator=(const RefCounted&) noexcept { return *this; }

    void ref() const noexcept { mRefCount.fetch_add(1, std::memory_order_relaxed); }

    void deref() const noexcept {
        if (mRefCount.fetch_sub(1, std::memory_order_acq_rel) == 1)
            delete static_cast<const T*>(this);
    }

    bool hasOneRef() const noexcept { return mRefCount.load(std::memory_order_acquire) == 1; }

protected:
    ~RefCounted() = default;

private:
    mutable std::atomic<unsigned> mRefCount {1};
};

template<typename T> class Ref {
public:
    Ref(const Ref& other) noexcept
        : mPtr(other.mPtr) {
        mPtr->ref();
    }

    Ref(Ref&& other) noexcept
        : mPtr(std::exchange(other.mPtr, nullptr)) {}

    ~Ref() {
        if (mPtr)
            mPtr->deref();
    }

    Ref& operator=(Ref other) noexcept {
        swap(other);
        return *this;
    }

    T* operator->() { return mPtr; }
    const T* operator->() const { return mPtr; }
    T& operator*() { return *mPtr; }
    const T& operator*() const { return *mPtr; }

    T& access() {
        ensureUnique();
        return *mPtr;
    }

    void swap(Ref& other) noexcept { std::swap(mPtr, other.mPtr); }

private:
    struct AdoptTag {};

    void ensureUnique() {
        if (!mPtr->hasOneRef())
            *this = copyRef();
    }

    Ref copyRef() const { return mPtr->copy(); }

    explicit Ref(T* ptr, AdoptTag)
        : mPtr(ptr) {}

    friend Ref<T> adoptRef<T>(T&);

    T* mPtr;
};

template<typename T> Ref<T> adoptRef(T& value) { return Ref<T>(&value, typename Ref<T>::AdoptTag {}); }
} // namespace Core::Style
