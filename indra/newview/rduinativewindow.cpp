#include "llviewerprecompiledheaders.h"
#include "rduinativewindow.h"

#if LL_WINDOWS

#include "indra_constants.h"
#include "llkeyboardwin32.h"
#include "llrender.h"
#include "llrendertarget.h"
#include "llwin32headers.h"
#include <windowsx.h>
#include <algorithm>
#include <cmath>

#endif

namespace rdui::viewer
{

#if LL_WINDOWS

namespace
{
    constexpr wchar_t WINDOW_CLASS[] = L"RadiaDetachedFloater";
    constexpr wchar_t RENDER_WINDOW_CLASS[] = L"RadiaDetachedFloaterRenderTarget";

    MASK currentModifiers()
    {
        MASK result = MASK_NONE;
        if (GetKeyState(VK_SHIFT) & 0x8000) result |= MASK_SHIFT;
        if (GetKeyState(VK_CONTROL) & 0x8000) result |= MASK_CONTROL;
        if (GetKeyState(VK_MENU) & 0x8000) result |= MASK_ALT;
        return result;
    }

    KEY translatedKey(WPARAM key, LPARAM data)
    {
        static LLKeyboardWin32 keyboard;
        KEY translated = static_cast<KEY>(key);
        const MASK extended = (data & (1LL << 24)) ? MASK_EXTENDED : MASK_NONE;
        keyboard.translateExtendedKey(static_cast<U16>(key), extended, &translated);
        return translated;
    }

    std::wstring wide(const std::string& value)
    {
        if (value.empty()) return {};
        const int count = MultiByteToWideChar(CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()), nullptr, 0);
        std::wstring result(std::max(0, count), L'\0');
        if (count > 0) MultiByteToWideChar(CP_UTF8, 0, value.c_str(), static_cast<int>(value.size()), result.data(), count);
        return result;
    }

    std::string monitorName(HMONITOR monitor)
    {
        MONITORINFOEXW info{};
        info.cbSize = sizeof(info);
        if (!monitor || !GetMonitorInfoW(monitor, &info)) return {};
        const int count = WideCharToMultiByte(CP_UTF8, 0, info.szDevice, -1, nullptr, 0, nullptr, nullptr);
        std::string result(count > 0 ? count : 0, '\0');
        if (count > 1)
        {
            WideCharToMultiByte(CP_UTF8, 0, info.szDevice, -1, result.data(), count, nullptr, nullptr);
            result.pop_back();
        }
        return result;
    }

    HCURSOR nativeCursor(ECursorType cursor)
    {
        switch (cursor)
        {
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

    class Win32NativeWindow final : public NativeWindow
    {
    public:
        Win32NativeWindow(const NativeRect& rect, const std::string& title, NativeWindowClient& client)
            : mClient(client)
        {
            registerClass();
            const std::wstring window_title = wide(title);
            mWindow = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_LAYERED, WINDOW_CLASS, window_title.c_str(),
                                      WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                                      rect.x, rect.y, std::max(1, rect.width), std::max(1, rect.height),
                                      nullptr, nullptr, GetModuleHandle(nullptr), this);
            if (!mWindow) return;

            // A layered HWND cannot also use CS_OWNDC. Render into an invisible
            // OpenGL HWND, then publish its premultiplied BGRA pixels to the
            // visible layered window in one UpdateLayeredWindow operation.
            mRenderWindow = CreateWindowExW(0, RENDER_WINDOW_CLASS, L"", WS_POPUP,
                                            0, 0, std::max(1, rect.width), std::max(1, rect.height),
                                            nullptr, nullptr, GetModuleHandle(nullptr), nullptr);
            mDC = mRenderWindow ? GetDC(mRenderWindow) : nullptr;
            HDC source_dc = wglGetCurrentDC();
            const int pixel_format = source_dc ? GetPixelFormat(source_dc) : 0;
            PIXELFORMATDESCRIPTOR descriptor{};
            if (!mDC || pixel_format <= 0
                || !DescribePixelFormat(source_dc, pixel_format, sizeof(descriptor), &descriptor)
                || !SetPixelFormat(mDC, pixel_format, &descriptor))
            {
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

        ~Win32NativeWindow() override
        {
            if (mWindow && GetCapture() == mWindow) ReleaseCapture();
            releaseLayerBuffer();
            releaseRenderTargetWithCurrentContext();
            if (mDC && mRenderWindow) ReleaseDC(mRenderWindow, mDC);
            if (mRenderWindow) DestroyWindow(mRenderWindow);
            if (mWindow) DestroyWindow(mWindow);
        }

        bool valid() const { return mWindow && mDC; }

        void show(bool activate) override
        {
            if (mWindow) ShowWindow(mWindow, activate ? SW_SHOWNORMAL : SW_SHOWNOACTIVATE);
        }

        void setVisible(bool visible) override
        {
            if (mWindow) ShowWindow(mWindow, visible ? SW_SHOWNOACTIVATE : SW_HIDE);
        }

        void setTitle(const std::string& title) override
        {
            if (mWindow) SetWindowTextW(mWindow, wide(title).c_str());
        }

        void pump() override
        {
            if (!mWindow) return;
            MSG message{};
            while (PeekMessageW(&message, mWindow, 0, 0, PM_REMOVE))
            {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            // The detached HWND intentionally does not activate during the
            // breakaway handoff. Windows may therefore deny or later remove
            // capture even though the physical drag is still active. Polling
            // keeps that drag continuous outside this window's message area.
            if (mDragging)
            {
                if (GetAsyncKeyState(VK_LBUTTON) & 0x8000)
                    updateDragPosition();
                else
                    endDrag();
            }
            if (mResizing && !(GetAsyncKeyState(VK_LBUTTON) & 0x8000))
            {
                mClient.dispatchNative({rdui::viewer::NativeInteractionLoss::Capture});
                endResize();
            }
        }

        void render() override
        {
            if (!mWindow || !mDC || !IsWindowVisible(mWindow)) return;
            HDC previous_dc = wglGetCurrentDC();
            HGLRC context = wglGetCurrentContext();
            if (!previous_dc || !context) return;
            gGL.flush();
            if (!wglMakeCurrent(mDC, context)) return;
            RECT client{};
            GetClientRect(mWindow, &client);
            const int width = client.right - client.left;
            const int height = client.bottom - client.top;
            if (width <= 0 || height <= 0 || !ensureLayerBuffer(width, height)
                || !ensureRenderTarget(width, height))
            {
                wglMakeCurrent(previous_dc, context);
                return;
            }
            SetWindowPos(mRenderWindow, nullptr, 0, 0, width, height,
                         SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOZORDER);
            mRenderTarget.bindTarget();
            mClient.paintNative(width, height, scale());
            GLint read_buffer = GL_COLOR_ATTACHMENT0;
            glGetIntegerv(GL_READ_BUFFER, &read_buffer);
            glReadBuffer(GL_COLOR_ATTACHMENT0);
            GLint pack_alignment = 4;
            glGetIntegerv(GL_PACK_ALIGNMENT, &pack_alignment);
            glPixelStorei(GL_PACK_ALIGNMENT, 4);
            glReadPixels(0, 0, width, height, GL_BGRA, GL_UNSIGNED_BYTE, mLayerBits);
            glPixelStorei(GL_PACK_ALIGNMENT, pack_alignment);
            glReadBuffer(read_buffer);
            mRenderTarget.flush();
            publishLayer(width, height);
            wglMakeCurrent(previous_dc, context);
        }

        void setScaleMultiplier(F32 multiplier) override
        {
            mScaleMultiplier = std::max(0.25f, multiplier);
        }

        void setLogicalSize(F32 width, F32 height) override
        {
            if (!mWindow) return;
            mLogicalWidth = std::max(1.f, width);
            mLogicalHeight = std::max(1.f, height);
            SetWindowPos(mWindow, nullptr, 0, 0,
                         std::max(1, static_cast<int>(std::round(mLogicalWidth * scale()))),
                         std::max(1, static_cast<int>(std::round(mLogicalHeight * scale()))),
                         SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOZORDER);
        }

        void setLogicalRect(const rdui::Rect& logical) override
        {
            if (!mWindow) return;
            mLogicalWidth = std::max(1.f, logical.w);
            mLogicalHeight = std::max(1.f, logical.h);
            if (!mResizing)
            {
                setLogicalSize(mLogicalWidth, mLogicalHeight);
                return;
            }

            const NativeRect native = nativeRectForLogicalResize(
                mResizeInitialRect, logical, scale());
            SetWindowPos(mWindow, nullptr, native.x, native.y, native.width, native.height,
                         SWP_NOACTIVATE | SWP_NOZORDER);
        }

        void beginDrag(F32 logical_x, F32 logical_y,
                       const std::optional<NativePoint>& requested_cursor) override
        {
            if (!mWindow) return;
            mLogicalDragOffset = {logical_x, logical_y};
            updateDragOffset();

            // A breakaway drag starts in another native window. Anchor the new
            // window to the live desktop cursor instead of relying on the last
            // viewer-relative position, which may already be stale at the
            // capture handoff.
            POINT cursor{};
            if (requested_cursor)
                SetCursorPos(requested_cursor->x, requested_cursor->y);
            if (GetCursorPos(&cursor))
            {
                SetWindowPos(mWindow, nullptr, cursor.x - mDragOffset.x, cursor.y - mDragOffset.y,
                             0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER);
            }

            mCursor = LoadCursor(nullptr, IDC_ARROW);
            SetCursor(mCursor);
            mDragging = true;
            SetCapture(mWindow);
        }

        void beginResize() override
        {
            if (!mWindow || mResizing) return;
            mResizeInitialRect = rect();
            mResizing = true;
            SetCapture(mWindow);
        }

        NativeRect rect() const override
        {
            RECT value{};
            if (mWindow) GetWindowRect(mWindow, &value);
            return {value.left, value.top, value.right - value.left, value.bottom - value.top};
        }

        std::string monitorId() const override
        {
            return mWindow ? monitorName(MonitorFromWindow(mWindow, MONITOR_DEFAULTTONEAREST)) : std::string();
        }

        F32 scale() const override { return mDpiScale * mScaleMultiplier; }

    private:
        static void registerClass()
        {
            static const bool registered = []
            {
                WNDCLASSEXW type{};
                type.cbSize = sizeof(type);
                type.style = CS_DBLCLKS;
                type.lpfnWndProc = windowProc;
                type.hInstance = GetModuleHandle(nullptr);
                type.hCursor = LoadCursor(nullptr, IDC_ARROW);
                type.lpszClassName = WINDOW_CLASS;
                const bool input_registered = RegisterClassExW(&type) != 0
                        || GetLastError() == ERROR_CLASS_ALREADY_EXISTS;

                WNDCLASSEXW render_type{};
                render_type.cbSize = sizeof(render_type);
                render_type.style = CS_OWNDC;
                render_type.lpfnWndProc = DefWindowProcW;
                render_type.hInstance = GetModuleHandle(nullptr);
                render_type.lpszClassName = RENDER_WINDOW_CLASS;
                const bool render_registered = RegisterClassExW(&render_type) != 0
                        || GetLastError() == ERROR_CLASS_ALREADY_EXISTS;
                return input_registered && render_registered;
            }();
            (void)registered;
        }

        void updateScale()
        {
            const UINT dpi = mWindow ? GetDpiForWindow(mWindow) : USER_DEFAULT_SCREEN_DPI;
            mDpiScale = std::max(0.25f, static_cast<F32>(dpi) / static_cast<F32>(USER_DEFAULT_SCREEN_DPI));
        }

        bool ensureLayerBuffer(int width, int height)
        {
            if (mLayerBitmap && width == mLayerWidth && height == mLayerHeight) return true;
            releaseLayerBuffer();

            BITMAPINFO info{};
            info.bmiHeader.biSize = sizeof(info.bmiHeader);
            info.bmiHeader.biWidth = width;
            // OpenGL and a positive-height DIB are both bottom-up, so the
            // readback lands in the exact orientation UpdateLayeredWindow needs.
            info.bmiHeader.biHeight = height;
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            info.bmiHeader.biCompression = BI_RGB;
            HDC screen = GetDC(nullptr);
            mLayerBitmap = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mLayerBits, nullptr, 0);
            mLayerDC = CreateCompatibleDC(screen);
            ReleaseDC(nullptr, screen);
            if (!mLayerBitmap || !mLayerDC)
            {
                releaseLayerBuffer();
                return false;
            }
            mLayerPreviousBitmap = SelectObject(mLayerDC, mLayerBitmap);
            mLayerWidth = width;
            mLayerHeight = height;
            return true;
        }

        bool ensureRenderTarget(int width, int height)
        {
            if (mRenderTarget.isComplete()
                && mRenderTarget.getWidth() == static_cast<U32>(width)
                && mRenderTarget.getHeight() == static_cast<U32>(height)) return true;
            mRenderTarget.release();
            return mRenderTarget.allocate(static_cast<U32>(width), static_cast<U32>(height), GL_RGBA8);
        }

        void releaseRenderTarget()
        {
            mRenderTarget.release();
        }

        void releaseRenderTargetWithCurrentContext()
        {
            if (!mRenderTarget.isComplete()) return;
            HDC previous_dc = wglGetCurrentDC();
            HGLRC context = wglGetCurrentContext();
            if (!context || !mDC || !wglMakeCurrent(mDC, context)) return;
            releaseRenderTarget();
            wglMakeCurrent(previous_dc, context);
        }

        void releaseLayerBuffer()
        {
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

        void publishLayer(int width, int height)
        {
            RECT window_rect{};
            GetWindowRect(mWindow, &window_rect);
            POINT destination{window_rect.left, window_rect.top};
            POINT source{0, 0};
            SIZE size{width, height};
            BLENDFUNCTION blend{AC_SRC_OVER, 0, 255, AC_SRC_ALPHA};
            HDC screen = GetDC(nullptr);
            UpdateLayeredWindow(mWindow, screen, &destination, &size, mLayerDC,
                                &source, 0, &blend, ULW_ALPHA);
            ReleaseDC(nullptr, screen);
        }

        void updateDragOffset()
        {
            RECT client{};
            GetClientRect(mWindow, &client);
            mDragOffset.x = static_cast<LONG>(std::round(mLogicalDragOffset.x * scale()));
            mDragOffset.y = static_cast<LONG>(std::round((client.bottom - client.top) - mLogicalDragOffset.y * scale()));
        }

        rdui::viewer::NativePointerInput pointerInput(rdui::viewer::NativePointerPhase phase,
                                                       rdui::viewer::NativePointerButton button,
                                                       LPARAM data, U8 click_count = 1) const
        {
            RECT client{};
            GetClientRect(mWindow, &client);
            F32 x = static_cast<F32>(GET_X_LPARAM(data)) / scale();
            F32 y = static_cast<F32>(client.bottom - GET_Y_LPARAM(data)) / scale();
            if (mResizing)
            {
                POINT cursor{};
                if (GetCursorPos(&cursor))
                {
                    x = static_cast<F32>(cursor.x - mResizeInitialRect.x) / scale();
                    y = static_cast<F32>(mResizeInitialRect.y + mResizeInitialRect.height - cursor.y) / scale();
                }
            }
            return {phase, x, y,
                    button, static_cast<U32>(currentModifiers()), click_count};
        }

        void dispatchPointer(rdui::viewer::NativePointerPhase phase,
                             rdui::viewer::NativePointerButton button,
                             LPARAM data, U8 click_count = 1)
        {
            const rdui::viewer::NativeInputDispatchResult result =
                mClient.dispatchNative({pointerInput(phase, button, data, click_count)});
            if (result.cursor) mCursor = nativeCursor(*result.cursor);
            SetCursor(mCursor);
        }

        void updateDragPosition()
        {
            POINT cursor{};
            if (!mWindow || !GetCursorPos(&cursor)) return;
            SetWindowPos(mWindow, nullptr, cursor.x - mDragOffset.x, cursor.y - mDragOffset.y,
                         0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER);
        }

        void endDrag()
        {
            if (!mDragging) return;
            mDragging = false;
            if (GetCapture() == mWindow) ReleaseCapture();
            mClient.nativeDragEnded();
        }

        void endResize()
        {
            if (!mResizing) return;
            mResizing = false;
            if (GetCapture() == mWindow) ReleaseCapture();
            mClient.nativeResizeEnded(mLogicalWidth, mLogicalHeight);
        }

        LRESULT handleMessage(UINT message, WPARAM wparam, LPARAM lparam)
        {
            switch (message)
            {
            case WM_CLOSE:
                mClient.closeNative();
                return 0;
            case WM_ERASEBKGND:
                return 1;
            case WM_SETCURSOR:
                SetCursor(mCursor);
                return TRUE;
            case WM_PAINT:
            {
                PAINTSTRUCT paint{};
                BeginPaint(mWindow, &paint);
                EndPaint(mWindow, &paint);
                return 0;
            }
            case WM_MOUSEMOVE:
                if (!mTrackingMouse)
                {
                    TRACKMOUSEEVENT tracking{sizeof(tracking), TME_LEAVE, mWindow, 0};
                    TrackMouseEvent(&tracking);
                    mTrackingMouse = true;
                }
                if (mDragging)
                {
                    updateDragPosition();
                }
                else dispatchPointer(rdui::viewer::NativePointerPhase::Move,
                                     rdui::viewer::NativePointerButton::NoButton, lparam);
                return 0;
            case WM_MOUSELEAVE:
                mTrackingMouse = false;
                mClient.dispatchNative({rdui::viewer::NativePointerInput{
                    rdui::viewer::NativePointerPhase::Leave}});
                return 0;
            case WM_LBUTTONDOWN:
                SetFocus(mWindow);
                dispatchPointer(rdui::viewer::NativePointerPhase::Down,
                                rdui::viewer::NativePointerButton::Left, lparam);
                return 0;
            case WM_LBUTTONDBLCLK:
                SetFocus(mWindow);
                dispatchPointer(rdui::viewer::NativePointerPhase::Down,
                                rdui::viewer::NativePointerButton::Left, lparam, 2);
                return 0;
            case WM_LBUTTONUP:
                if (mDragging) endDrag();
                else
                {
                    dispatchPointer(rdui::viewer::NativePointerPhase::Up,
                                    rdui::viewer::NativePointerButton::Left, lparam);
                    if (mResizing) endResize();
                }
                return 0;
            case WM_RBUTTONDOWN:
                dispatchPointer(rdui::viewer::NativePointerPhase::Down,
                                rdui::viewer::NativePointerButton::Right, lparam);
                return 0;
            case WM_RBUTTONUP:
                dispatchPointer(rdui::viewer::NativePointerPhase::Up,
                                rdui::viewer::NativePointerButton::Right, lparam);
                return 0;
            case WM_MBUTTONDOWN:
                dispatchPointer(rdui::viewer::NativePointerPhase::Down,
                                rdui::viewer::NativePointerButton::Middle, lparam);
                return 0;
            case WM_MBUTTONUP:
                dispatchPointer(rdui::viewer::NativePointerPhase::Up,
                                rdui::viewer::NativePointerButton::Middle, lparam);
                return 0;
            case WM_MOUSEWHEEL:
            case WM_MOUSEHWHEEL:
            {
                POINT point{GET_X_LPARAM(lparam), GET_Y_LPARAM(lparam)};
                ScreenToClient(mWindow, &point);
                RECT client{};
                GetClientRect(mWindow, &client);
                const F32 delta = static_cast<F32>(GET_WHEEL_DELTA_WPARAM(wparam)) / WHEEL_DELTA;
                mClient.dispatchNative({rdui::viewer::NativeScrollInput{
                    static_cast<S32>(std::round(point.x / scale())),
                    static_cast<S32>(std::round((client.bottom - point.y) / scale())),
                    message == WM_MOUSEHWHEEL ? delta : 0.f,
                    message == WM_MOUSEWHEEL ? delta : 0.f,
                    static_cast<U32>(currentModifiers())}});
                return 0;
            }
            case WM_KEYDOWN:
            case WM_SYSKEYDOWN:
                mClient.dispatchNative({rdui::viewer::NativeKeyInput{translatedKey(wparam, lparam),
                    static_cast<U32>(currentModifiers()), true, (lparam & (1LL << 30)) != 0}});
                return 0;
            case WM_KEYUP:
            case WM_SYSKEYUP:
                mClient.dispatchNative({rdui::viewer::NativeKeyInput{translatedKey(wparam, lparam),
                    static_cast<U32>(currentModifiers()), false, false}});
                return 0;
            case WM_CHAR:
            {
                const U16 unit = static_cast<U16>(wparam);
                if (unit >= 0xD800 && unit <= 0xDBFF)
                {
                    mHighSurrogate = unit;
                    return 0;
                }
                U32 codepoint = unit;
                if (unit >= 0xDC00 && unit <= 0xDFFF && mHighSurrogate)
                    codepoint = 0x10000u + ((static_cast<U32>(mHighSurrogate) - 0xD800u) << 10)
                              + (static_cast<U32>(unit) - 0xDC00u);
                mHighSurrogate = 0;
                mClient.dispatchNative({rdui::viewer::NativeCharacterInput{
                    codepoint, static_cast<U32>(currentModifiers())}});
                return 0;
            }
            case WM_KILLFOCUS:
                mClient.dispatchNative({rdui::viewer::NativeInteractionLoss::Focus});
                return 0;
            case WM_CAPTURECHANGED:
                if (mDragging && reinterpret_cast<HWND>(lparam) != mWindow)
                {
                    if (!(GetAsyncKeyState(VK_LBUTTON) & 0x8000)) endDrag();
                }
                else
                {
                    mClient.dispatchNative({rdui::viewer::NativeInteractionLoss::Capture});
                    if (mResizing && reinterpret_cast<HWND>(lparam) != mWindow) endResize();
                }
                return 0;
            case WM_DPICHANGED:
            {
                const UINT dpi = HIWORD(wparam);
                mDpiScale = std::max(0.25f, static_cast<F32>(dpi) / static_cast<F32>(USER_DEFAULT_SCREEN_DPI));
                const RECT& suggested = *reinterpret_cast<RECT*>(lparam);
                const int width = mLogicalWidth > 0.f
                    ? std::max(1, static_cast<int>(std::round(mLogicalWidth * scale())))
                    : suggested.right - suggested.left;
                const int height = mLogicalHeight > 0.f
                    ? std::max(1, static_cast<int>(std::round(mLogicalHeight * scale())))
                    : suggested.bottom - suggested.top;
                SetWindowPos(mWindow, nullptr, suggested.left, suggested.top,
                             width, height,
                             SWP_NOACTIVATE | SWP_NOZORDER);
                if (mDragging) updateDragOffset();
                return 0;
            }
            default:
                return DefWindowProcW(mWindow, message, wparam, lparam);
            }
        }

        static LRESULT CALLBACK windowProc(HWND window, UINT message, WPARAM wparam, LPARAM lparam)
        {
            Win32NativeWindow* self = reinterpret_cast<Win32NativeWindow*>(GetWindowLongPtrW(window, GWLP_USERDATA));
            if (message == WM_NCCREATE)
            {
                self = static_cast<Win32NativeWindow*>(reinterpret_cast<CREATESTRUCTW*>(lparam)->lpCreateParams);
                self->mWindow = window;
                SetWindowLongPtrW(window, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(self));
            }
            return self ? self->handleMessage(message, wparam, lparam) : DefWindowProcW(window, message, wparam, lparam);
        }

        NativeWindowClient& mClient;
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
        NativeRect mResizeInitialRect;
        struct { F32 x = 0.f; F32 y = 0.f; } mLogicalDragOffset;
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
}

std::unique_ptr<NativeWindow> NativeWindow::create(
    const NativeRect& rect, const std::string& title, NativeWindowClient& client)
{
    auto window = std::make_unique<Win32NativeWindow>(rect, title, client);
    return window->valid() ? std::move(window) : nullptr;
}

bool NativeWindow::placementVisible(const NativeRect& rect, const std::string& monitor_id)
{
    RECT native{rect.x, rect.y, rect.x + rect.width, rect.y + rect.height};
    HMONITOR monitor = MonitorFromRect(&native, MONITOR_DEFAULTTONULL);
    return monitor && (monitor_id.empty() || monitorName(monitor) == monitor_id);
}

#else

std::unique_ptr<NativeWindow> NativeWindow::create(
    const NativeRect&, const std::string&, NativeWindowClient&)
{
    return nullptr;
}

bool NativeWindow::placementVisible(const NativeRect&, const std::string&)
{
    return false;
}

#endif

namespace
{
    class DefaultNativeWindowFactory final : public NativeWindowFactory
    {
        public:
            std::unique_ptr<NativeWindow> create(
                const NativeRect& rect, const std::string& title,
                NativeWindowClient& client) override
            {
                return NativeWindow::create(rect, title, client);
            }

            bool placementVisible(const NativeRect& rect,
                                  const std::string& monitor_id) const override
            {
                return NativeWindow::placementVisible(rect, monitor_id);
            }
    };
}

NativeWindowFactory& defaultNativeWindowFactory()
{
    static DefaultNativeWindowFactory factory;
    return factory;
}

}
