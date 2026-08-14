/**
 * @file auxiliarywindow.cpp
 * @brief Implements the platform seam for non-primary viewer windows.
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
#include "auxiliarywindow.h"

#if LL_WINDOWS
#include <algorithm>
#include <cmath>
#include <windowsx.h>
#include "indra_constants.h"
#if !LL_SDL_WINDOW
#include "llkeyboardwin32.h"
#endif
#include "llrender.h"
#include "llrendertarget.h"
#include "llwin32headers.h"
#endif

#if LL_WINDOWS
namespace {
constexpr wchar_t kWindowClassName[] = L"RadiaDetachedFloater";
constexpr wchar_t kRenderWindowClassName[] = L"RadiaDetachedFloaterRenderTarget";

MASK currentModifiers() {
    MASK result = MASK_NONE;
    if (GetKeyState(VK_SHIFT) & 0x8000) result |= MASK_SHIFT;
    if (GetKeyState(VK_CONTROL) & 0x8000) result |= MASK_CONTROL;
    if (GetKeyState(VK_MENU) & 0x8000) result |= MASK_ALT;
    return result;
}

KEY translatedKey(WPARAM key, LPARAM data) {
#if LL_SDL_WINDOW
    return static_cast<KEY>(key);
#else
    static LLKeyboardWin32 keyboard;
    KEY translated = static_cast<KEY>(key);
    const MASK extended = (data & (1LL << 24)) ? MASK_EXTENDED : MASK_NONE;
    keyboard.translateExtendedKey(static_cast<U16>(key), extended, &translated);
    return translated;
#endif
}

std::wstring wide(const std::string& value) {
    if (value.empty()) return {};
    const int count = MultiByteToWideChar(CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()), nullptr, 0);
    std::wstring result(std::max(0, count), L'\0');
    if (count > 0) MultiByteToWideChar(CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()), result.data(), count);
    return result;
}

HCURSOR nativeCursor(ECursorType cursor) {
    switch (cursor) {
        case UI_CURSOR_HAND: return LoadCursor(nullptr, IDC_HAND);
        case UI_CURSOR_IBEAM: return LoadCursor(nullptr, IDC_IBEAM);
        case UI_CURSOR_CROSS: return LoadCursor(nullptr, IDC_CROSS);
        case UI_CURSOR_WAIT: return LoadCursor(nullptr, IDC_WAIT);
        case UI_CURSOR_WORKING: return LoadCursor(nullptr, IDC_APPSTARTING);
        case UI_CURSOR_SIZEWE: return LoadCursor(nullptr, IDC_SIZEWE);
        case UI_CURSOR_SIZENS: return LoadCursor(nullptr, IDC_SIZENS);
        case UI_CURSOR_SIZENESW: return LoadCursor(nullptr, IDC_SIZENESW);
        case UI_CURSOR_SIZENWSE: return LoadCursor(nullptr, IDC_SIZENWSE);
        case UI_CURSOR_SIZEALL: return LoadCursor(nullptr, IDC_SIZEALL);
        case UI_CURSOR_NO: return LoadCursor(nullptr, IDC_NO);
        default: return LoadCursor(nullptr, IDC_ARROW);
    }
}

struct PointerInput {
    F32 x = 0.f;
    F32 y = 0.f;
    AuxiliaryPointerButton button = AuxiliaryPointerButton::NoButton;
    U8 clickCount = 1;
};

class Win32AuxiliaryWindow final : public AuxiliaryWindow {
public:
    Win32AuxiliaryWindow(const AuxiliaryWindowRect& rect, const std::string& title, AuxiliaryWindowClient& client) : mClient(client) {
        registerClass();
        const std::wstring windowTitle = wide(title);
        mWindow =
            CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_LAYERED, kWindowClassName, windowTitle.c_str(), WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                            rect.x, rect.y, std::max(1, rect.width), std::max(1, rect.height), nullptr, nullptr, GetModuleHandle(nullptr), this);
        if (!mWindow) return;

        mRenderWindow = CreateWindowExW(0, kRenderWindowClassName, L"", WS_POPUP, 0, 0, std::max(1, rect.width), std::max(1, rect.height), nullptr,
                                        nullptr, GetModuleHandle(nullptr), nullptr);
        mDC = mRenderWindow ? GetDC(mRenderWindow) : nullptr;
        HDC sourceDC = wglGetCurrentDC();
        const int pixelFormat = sourceDC ? GetPixelFormat(sourceDC) : 0;
        PIXELFORMATDESCRIPTOR descriptor{};
        if (!mDC
            || pixelFormat <= 0
            || !DescribePixelFormat(sourceDC, pixelFormat, sizeof(descriptor), &descriptor)
            || !SetPixelFormat(mDC, pixelFormat, &descriptor)) {
            if (mDC) ReleaseDC(mRenderWindow, mDC);
            mDC = nullptr;
            if (mRenderWindow) DestroyWindow(mRenderWindow);
            mRenderWindow = nullptr;
            DestroyWindow(mWindow);
            mWindow = nullptr;
            return;
        }
        updateScale();
    }

    ~Win32AuxiliaryWindow() override {
        if (mWindow && GetCapture() == mWindow) ReleaseCapture();
        releaseLayerBuffer();
        releaseRenderTargetWithCurrentContext();
        if (mDC && mRenderWindow) ReleaseDC(mRenderWindow, mDC);
        if (mRenderWindow) DestroyWindow(mRenderWindow);
        if (mWindow) DestroyWindow(mWindow);
    }

    bool valid() const { return mWindow && mDC; }

    void show(bool activate) override {
        if (mWindow) ShowWindow(mWindow, activate ? SW_SHOWNORMAL : SW_SHOWNOACTIVATE);
    }

    void setVisible(bool visible) override {
        if (mWindow) ShowWindow(mWindow, visible ? SW_SHOWNOACTIVATE : SW_HIDE);
    }

    void setTitle(const std::string& title) override {
        if (mWindow) SetWindowTextW(mWindow, wide(title).c_str());
    }

    void pump() override {
        if (!mWindow) return;
        MSG message{};
        while (PeekMessageW(&message, mWindow, 0, 0, PM_REMOVE)) {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        if (mDragging) {
            if (GetAsyncKeyState(VK_LBUTTON) & 0x8000) updateDragPosition();
            else endDrag();
        }
        if (mResizing && !(GetAsyncKeyState(VK_LBUTTON) & 0x8000)) {
            mClient.interactionLost(AuxiliaryInteractionLoss::Capture);
            endResize();
        }
    }

    void render() override {
        if (!mWindow || !mDC || !IsWindowVisible(mWindow)) return;
        HDC previousDC = wglGetCurrentDC();
        HGLRC context = wglGetCurrentContext();
        if (!previousDC || !context) return;
        gGL.flush();
        if (!wglMakeCurrent(mDC, context)) return;
        RECT client{};
        GetClientRect(mWindow, &client);
        const int width = client.right - client.left;
        const int height = client.bottom - client.top;
        if (width <= 0 || height <= 0 || !ensureLayerBuffer(width, height) || !ensureRenderTarget(width, height)) {
            wglMakeCurrent(previousDC, context);
            return;
        }
        SetWindowPos(mRenderWindow, nullptr, 0, 0, width, height, SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOZORDER);
        mRenderTarget.bindTarget();
        mClient.paint(width, height, scale());
        GLint readBuffer = GL_COLOR_ATTACHMENT0;
        glGetIntegerv(GL_READ_BUFFER, &readBuffer);
        glReadBuffer(GL_COLOR_ATTACHMENT0);
        GLint packAlignment = 4;
        glGetIntegerv(GL_PACK_ALIGNMENT, &packAlignment);
        glPixelStorei(GL_PACK_ALIGNMENT, 4);
        glReadPixels(0, 0, width, height, GL_BGRA, GL_UNSIGNED_BYTE, mLayerBits);
        glPixelStorei(GL_PACK_ALIGNMENT, packAlignment);
        glReadBuffer(readBuffer);
        mRenderTarget.flush();
        publishLayer(width, height);
        wglMakeCurrent(previousDC, context);
    }

    void setScaleMultiplier(F32 multiplier) override { mScaleMultiplier = std::max(0.25f, multiplier); }

    void setLogicalSize(F32 width, F32 height) override {
        if (!mWindow) return;
        mLogicalWidth = std::max(1.f, width);
        mLogicalHeight = std::max(1.f, height);
        SetWindowPos(mWindow, nullptr, 0, 0, std::max(1, static_cast<int>(std::round(mLogicalWidth * scale()))),
                     std::max(1, static_cast<int>(std::round(mLogicalHeight * scale()))), SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOZORDER);
    }

    void setLogicalRect(const AuxiliaryLogicalRect& logical) override {
        if (!mWindow) return;
        mLogicalWidth = std::max(1.f, logical.width);
        mLogicalHeight = std::max(1.f, logical.height);
        if (!mResizing) {
            setLogicalSize(mLogicalWidth, mLogicalHeight);
            return;
        }

        const AuxiliaryWindowRect native = auxiliaryWindowRectForLogicalResize(mResizeInitialRect, logical, scale());
        SetWindowPos(mWindow, nullptr, native.x, native.y, native.width, native.height, SWP_NOACTIVATE | SWP_NOZORDER);
    }

    void beginDrag(F32 logicalX, F32 logicalY, const std::optional<AuxiliaryWindowPoint>& requestedCursor) override {
        if (!mWindow) return;
        mLogicalDragOffset = {logicalX, logicalY};
        updateDragOffset();

        POINT cursor{};
        if (requestedCursor) SetCursorPos(requestedCursor->x, requestedCursor->y);
        if (GetCursorPos(&cursor))
            SetWindowPos(mWindow, nullptr, cursor.x - mDragOffset.x, cursor.y - mDragOffset.y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER);

        mCursor = LoadCursor(nullptr, IDC_ARROW);
        SetCursor(mCursor);
        mDragging = true;
        SetCapture(mWindow);
    }

    void beginResize() override {
        if (!mWindow || mResizing) return;
        mResizeInitialRect = rect();
        mResizing = true;
        SetCapture(mWindow);
    }

    AuxiliaryWindowRect rect() const override {
        RECT value{};
        if (mWindow) GetWindowRect(mWindow, &value);
        return {value.left, value.top, value.right - value.left, value.bottom - value.top};
    }

    F32 scale() const override { return mDpiScale * mScaleMultiplier; }

private:
    static void registerClass() {
        static const bool registered = [] {
            WNDCLASSEXW type{};
            type.cbSize = sizeof(type);
            type.style = CS_DBLCLKS;
            type.lpfnWndProc = windowProc;
            type.hInstance = GetModuleHandle(nullptr);
            type.hCursor = LoadCursor(nullptr, IDC_ARROW);
            type.lpszClassName = kWindowClassName;
            const bool inputRegistered = RegisterClassExW(&type) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS;

            WNDCLASSEXW renderType{};
            renderType.cbSize = sizeof(renderType);
            renderType.style = CS_OWNDC;
            renderType.lpfnWndProc = DefWindowProcW;
            renderType.hInstance = GetModuleHandle(nullptr);
            renderType.lpszClassName = kRenderWindowClassName;
            const bool renderRegistered = RegisterClassExW(&renderType) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS;
            return inputRegistered && renderRegistered;
        }();
        (void)registered;
    }

    void updateScale() {
        const UINT dpi = mWindow ? GetDpiForWindow(mWindow) : USER_DEFAULT_SCREEN_DPI;
        mDpiScale = std::max(0.25f, static_cast<F32>(dpi) / static_cast<F32>(USER_DEFAULT_SCREEN_DPI));
    }

    bool ensureLayerBuffer(int width, int height) {
        if (mLayerBitmap && width == mLayerWidth && height == mLayerHeight) return true;
        releaseLayerBuffer();

        BITMAPINFO info{};
        info.bmiHeader.biSize = sizeof(info.bmiHeader);
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = height;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        HDC screen = GetDC(nullptr);
        mLayerBitmap = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mLayerBits, nullptr, 0);
        mLayerDC = CreateCompatibleDC(screen);
        ReleaseDC(nullptr, screen);
        if (!mLayerBitmap || !mLayerDC) {
            releaseLayerBuffer();
            return false;
        }
        mLayerPreviousBitmap = SelectObject(mLayerDC, mLayerBitmap);
        mLayerWidth = width;
        mLayerHeight = height;
        return true;
    }

    bool ensureRenderTarget(int width, int height) {
        if (mRenderTarget.isComplete()
            && mRenderTarget.getWidth() == static_cast<U32>(width)
            && mRenderTarget.getHeight() == static_cast<U32>(height))
            return true;
        mRenderTarget.release();
        return mRenderTarget.allocate(static_cast<U32>(width), static_cast<U32>(height), GL_RGBA8);
    }

    void releaseRenderTargetWithCurrentContext() {
        if (!mRenderTarget.isComplete()) return;
        HDC previousDC = wglGetCurrentDC();
        HGLRC context = wglGetCurrentContext();
        if (!context || !mDC || !wglMakeCurrent(mDC, context)) return;
        mRenderTarget.release();
        wglMakeCurrent(previousDC, context);
    }

    void releaseLayerBuffer() {
        if (mLayerDC && mLayerPreviousBitmap) SelectObject(mLayerDC, mLayerPreviousBitmap);
        if (mLayerBitmap) DeleteObject(mLayerBitmap);
        if (mLayerDC) DeleteDC(mLayerDC);
        mLayerBitmap = nullptr;
        mLayerDC = nullptr;
        mLayerPreviousBitmap = nullptr;
        mLayerBits = nullptr;
        mLayerWidth = 0;
        mLayerHeight = 0;
    }

    void publishLayer(int width, int height) {
        RECT windowRect{};
        GetWindowRect(mWindow, &windowRect);
        POINT destination{windowRect.left, windowRect.top};
        POINT source{0, 0};
        SIZE size{width, height};
        BLENDFUNCTION blend{AC_SRC_OVER, 0, 255, AC_SRC_ALPHA};
        HDC screen = GetDC(nullptr);
        UpdateLayeredWindow(mWindow, screen, &destination, &size, mLayerDC, &source, 0, &blend, ULW_ALPHA);
        ReleaseDC(nullptr, screen);
    }

    void updateDragOffset() {
        RECT client{};
        GetClientRect(mWindow, &client);
        mDragOffset.x = static_cast<LONG>(std::round(mLogicalDragOffset.x * scale()));
        mDragOffset.y = static_cast<LONG>(std::round((client.bottom - client.top) - mLogicalDragOffset.y * scale()));
    }

    PointerInput pointerInput(AuxiliaryPointerButton button, LPARAM data, U8 clickCount = 1) const {
        RECT client{};
        GetClientRect(mWindow, &client);
        F32 x = static_cast<F32>(GET_X_LPARAM(data)) / scale();
        F32 y = static_cast<F32>(client.bottom - GET_Y_LPARAM(data)) / scale();
        if (mResizing) {
            POINT cursor{};
            if (GetCursorPos(&cursor)) {
                x = static_cast<F32>(cursor.x - mResizeInitialRect.x) / scale();
                y = static_cast<F32>(mResizeInitialRect.y + mResizeInitialRect.height - cursor.y) / scale();
            }
        }
        return {x, y, button, clickCount};
    }

    void applyCursor(const AuxiliaryInputResult& result) {
        if (result.cursor) mCursor = nativeCursor(*result.cursor);
        SetCursor(mCursor);
    }

    void dispatchPointer(bool down, AuxiliaryPointerButton button, LPARAM data, U8 clickCount = 1) {
        const PointerInput input = pointerInput(button, data, clickCount);
        const AuxiliaryInputResult result = down ? mClient.pointerDown(input.x, input.y, input.button, currentModifiers(), input.clickCount, 0.f, 0.f)
                                                 : mClient.pointerUp(input.x, input.y, input.button, currentModifiers(), input.clickCount, 0.f, 0.f);
        applyCursor(result);
    }

    void dispatchPointerMove(LPARAM data) {
        const PointerInput input = pointerInput(AuxiliaryPointerButton::NoButton, data);
        applyCursor(mClient.pointerMove(input.x, input.y, input.button, currentModifiers(), input.clickCount, 0.f, 0.f));
    }

    void updateDragPosition() {
        POINT cursor{};
        if (!mWindow || !GetCursorPos(&cursor)) return;
        SetWindowPos(mWindow, nullptr, cursor.x - mDragOffset.x, cursor.y - mDragOffset.y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER);
    }

    void endDrag() {
        if (!mDragging) return;
        mDragging = false;
        if (GetCapture() == mWindow) ReleaseCapture();
        mClient.dragEnded();
    }

    void endResize() {
        if (!mResizing) return;
        mResizing = false;
        if (GetCapture() == mWindow) ReleaseCapture();
        mClient.resizeEnded(mLogicalWidth, mLogicalHeight);
    }

    LRESULT handleMessage(UINT message, WPARAM wparam, LPARAM lparam) {
        switch (message) {
            case WM_CLOSE: mClient.closeRequested(); return 0;
            case WM_ERASEBKGND: return 1;
            case WM_SETCURSOR: SetCursor(mCursor); return TRUE;
            case WM_PAINT: {
                PAINTSTRUCT paint{};
                BeginPaint(mWindow, &paint);
                EndPaint(mWindow, &paint);
                return 0;
            }
            case WM_MOUSEMOVE:
                if (!mTrackingMouse) {
                    TRACKMOUSEEVENT tracking{sizeof(tracking), TME_LEAVE, mWindow, 0};
                    TrackMouseEvent(&tracking);
                    mTrackingMouse = true;
                }
                if (mDragging) updateDragPosition();
                else dispatchPointerMove(lparam);
                return 0;
            case WM_MOUSELEAVE:
                mTrackingMouse = false;
                mClient.pointerLeave();
                return 0;
            case WM_LBUTTONDOWN:
                SetFocus(mWindow);
                dispatchPointer(true, AuxiliaryPointerButton::Left, lparam);
                return 0;
            case WM_LBUTTONDBLCLK:
                SetFocus(mWindow);
                dispatchPointer(true, AuxiliaryPointerButton::Left, lparam, 2);
                return 0;
            case WM_LBUTTONUP:
                if (mDragging) endDrag();
                else {
                    dispatchPointer(false, AuxiliaryPointerButton::Left, lparam);
                    if (mResizing) endResize();
                }
                return 0;
            case WM_RBUTTONDOWN: dispatchPointer(true, AuxiliaryPointerButton::Right, lparam); return 0;
            case WM_RBUTTONUP: dispatchPointer(false, AuxiliaryPointerButton::Right, lparam); return 0;
            case WM_MBUTTONDOWN: dispatchPointer(true, AuxiliaryPointerButton::Middle, lparam); return 0;
            case WM_MBUTTONUP: dispatchPointer(false, AuxiliaryPointerButton::Middle, lparam); return 0;
            case WM_MOUSEWHEEL:
            case WM_MOUSEHWHEEL: {
                POINT point{GET_X_LPARAM(lparam), GET_Y_LPARAM(lparam)};
                ScreenToClient(mWindow, &point);
                RECT client{};
                GetClientRect(mWindow, &client);
                const F32 delta = static_cast<F32>(GET_WHEEL_DELTA_WPARAM(wparam)) / WHEEL_DELTA;
                const AuxiliaryInputResult result =
                    mClient.scroll(static_cast<S32>(std::round(point.x / scale())), static_cast<S32>(std::round((client.bottom - point.y) / scale())),
                                   message == WM_MOUSEHWHEEL ? delta : 0.f, message == WM_MOUSEWHEEL ? delta : 0.f, currentModifiers());
                applyCursor(result);
                return 0;
            }
            case WM_KEYDOWN:
            case WM_SYSKEYDOWN:
                applyCursor(mClient.keyDown(translatedKey(wparam, lparam), currentModifiers(), (lparam & (1LL << 30)) != 0));
                return 0;
            case WM_KEYUP:
            case WM_SYSKEYUP: applyCursor(mClient.keyUp(translatedKey(wparam, lparam), currentModifiers())); return 0;
            case WM_CHAR: {
                const U16 unit = static_cast<U16>(wparam);
                if (unit >= 0xD800 && unit <= 0xDBFF) {
                    mHighSurrogate = unit;
                    return 0;
                }
                U32 codepoint = unit;
                if (unit >= 0xDC00 && unit <= 0xDFFF && mHighSurrogate)
                    codepoint = 0x10000u + ((static_cast<U32>(mHighSurrogate) - 0xD800u) << 10) + (static_cast<U32>(unit) - 0xDC00u);
                mHighSurrogate = 0;
                applyCursor(mClient.character(codepoint, currentModifiers()));
                return 0;
            }
            case WM_KILLFOCUS: mClient.interactionLost(AuxiliaryInteractionLoss::Focus); return 0;
            case WM_CAPTURECHANGED:
                if (mDragging && reinterpret_cast<HWND>(lparam) != mWindow) {
                    if (!(GetAsyncKeyState(VK_LBUTTON) & 0x8000)) endDrag();
                } else {
                    mClient.interactionLost(AuxiliaryInteractionLoss::Capture);
                    if (mResizing && reinterpret_cast<HWND>(lparam) != mWindow) endResize();
                }
                return 0;
            case WM_DPICHANGED: {
                const UINT dpi = HIWORD(wparam);
                mDpiScale = std::max(0.25f, static_cast<F32>(dpi) / static_cast<F32>(USER_DEFAULT_SCREEN_DPI));
                const RECT& suggested = *reinterpret_cast<RECT*>(lparam);
                const int width =
                    mLogicalWidth > 0.f ? std::max(1, static_cast<int>(std::round(mLogicalWidth * scale()))) : suggested.right - suggested.left;
                const int height =
                    mLogicalHeight > 0.f ? std::max(1, static_cast<int>(std::round(mLogicalHeight * scale()))) : suggested.bottom - suggested.top;
                SetWindowPos(mWindow, nullptr, suggested.left, suggested.top, width, height, SWP_NOACTIVATE | SWP_NOZORDER);
                if (mDragging) updateDragOffset();
                return 0;
            }
            default: return DefWindowProcW(mWindow, message, wparam, lparam);
        }
    }

    static LRESULT CALLBACK windowProc(HWND window, UINT message, WPARAM wparam, LPARAM lparam) {
        Win32AuxiliaryWindow* self = reinterpret_cast<Win32AuxiliaryWindow*>(GetWindowLongPtrW(window, GWLP_USERDATA));
        if (message == WM_NCCREATE) {
            self = static_cast<Win32AuxiliaryWindow*>(reinterpret_cast<CREATESTRUCTW*>(lparam)->lpCreateParams);
            self->mWindow = window;
            SetWindowLongPtrW(window, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(self));
        }
        return self ? self->handleMessage(message, wparam, lparam) : DefWindowProcW(window, message, wparam, lparam);
    }

    AuxiliaryWindowClient& mClient;
    HWND mWindow = nullptr;
    HWND mRenderWindow = nullptr;
    HDC mDC = nullptr;
    HDC mLayerDC = nullptr;
    HBITMAP mLayerBitmap = nullptr;
    HGDIOBJ mLayerPreviousBitmap = nullptr;
    void* mLayerBits = nullptr;
    int mLayerWidth = 0;
    int mLayerHeight = 0;
    LLRenderTarget mRenderTarget;
    POINT mDragOffset{};
    AuxiliaryWindowRect mResizeInitialRect;
    struct {
        F32 x = 0.f;
        F32 y = 0.f;
    } mLogicalDragOffset;
    F32 mDpiScale = 1.f;
    F32 mScaleMultiplier = 1.f;
    F32 mLogicalWidth = 0.f;
    F32 mLogicalHeight = 0.f;
    bool mDragging = false;
    bool mResizing = false;
    bool mTrackingMouse = false;
    HCURSOR mCursor = LoadCursor(nullptr, IDC_ARROW);
    U16 mHighSurrogate = 0;
};
} // namespace
#endif

namespace {
class DefaultAuxiliaryWindowFactory final : public AuxiliaryWindowFactory {
public:
    std::unique_ptr<AuxiliaryWindow> create(const AuxiliaryWindowRect& rect, const std::string& title, AuxiliaryWindowClient& client) override {
#if LL_WINDOWS
        auto window = std::make_unique<Win32AuxiliaryWindow>(rect, title, client);
        return window->valid() ? std::move(window) : nullptr;
#else
        return nullptr;
#endif
    }

    bool placementVisible(const AuxiliaryWindowRect& rect) const override {
#if LL_WINDOWS
        RECT native{rect.x, rect.y, rect.x + rect.width, rect.y + rect.height};
        return MonitorFromRect(&native, MONITOR_DEFAULTTONULL) != nullptr;
#else
        return false;
#endif
    }
};
} // namespace

AuxiliaryWindowFactory& defaultAuxiliaryWindowFactory() {
    static DefaultAuxiliaryWindowFactory factory;
    return factory;
}
