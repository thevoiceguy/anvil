#include "flutter_window.h"

#include <flutter/method_result_functions.h>
#include <flutter/standard_method_codec.h>

#include <memory>
#include <optional>

#include "flutter/generated_plugin_registrant.h"
#include "instance.h"

FlutterWindow::FlutterWindow(const flutter::DartProject& project, bool hidden)
    : project_(project), hidden_(hidden) {}

FlutterWindow::~FlutterWindow() {}

bool FlutterWindow::OnCreate() {
  if (!Win32Window::OnCreate()) {
    return false;
  }

  RECT frame = GetClientArea();

  // The size here must match the window dimensions to avoid unnecessary surface
  // creation / destruction in the startup path.
  flutter_controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project_);
  // Ensure that basic setup of the controller was successful.
  if (!flutter_controller_->engine() || !flutter_controller_->view()) {
    return false;
  }
  RegisterPlugins(flutter_controller_->engine());
  SetChildContent(flutter_controller_->view()->GetNativeWindow());

  instance_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      flutter_controller_->engine()->messenger(), "anvil/instance",
      &flutter::StandardMethodCodec::GetInstance());
  ::SetPropW(GetHandle(), single_instance::kWindowProp,
             reinterpret_cast<HANDLE>(1));

  flutter_controller_->engine()->SetNextFrameCallback([&]() {
    if (!hidden_) {
      this->Show();
    }
  });

  // Flutter can complete the first frame before the "show window" callback is
  // registered. The following call ensures a frame is pending to ensure the
  // window is shown. It is a no-op if the first frame hasn't completed yet.
  flutter_controller_->ForceRedraw();

  return true;
}

void FlutterWindow::OnDestroy() {
  ::RemovePropW(GetHandle(), single_instance::kWindowProp);
  instance_ = nullptr;
  if (flutter_controller_) {
    flutter_controller_ = nullptr;
  }

  Win32Window::OnDestroy();
}

LRESULT
FlutterWindow::MessageHandler(HWND hwnd, UINT const message,
                              WPARAM const wparam,
                              LPARAM const lparam) noexcept {
  switch (message) {
    case WM_COPYDATA: {
      auto data = reinterpret_cast<const COPYDATASTRUCT*>(lparam);
      if (instance_ && data && data->dwData == single_instance::kForwarded) {
        flutter::EncodableList args;
        for (const auto& arg : single_instance::Arguments(data)) {
          args.push_back(flutter::EncodableValue(arg));
        }
        instance_->InvokeMethod(
            "started",
            std::make_unique<flutter::EncodableValue>(std::move(args)));
        return TRUE;
      }
      break;
    }
    case WM_QUERYENDSESSION:
      return TRUE;
    case WM_ENDSESSION:
      if (wparam) {
        SessionEnding();
      }
      return 0;
  }

  // Give Flutter, including plugins, an opportunity to handle window messages.
  if (flutter_controller_) {
    std::optional<LRESULT> result =
        flutter_controller_->HandleTopLevelWindowProc(hwnd, message, wparam,
                                                      lparam);
    if (result) {
      return *result;
    }
  }

  switch (message) {
    case WM_FONTCHANGE:
      flutter_controller_->engine()->ReloadSystemFonts();
      break;
  }

  return Win32Window::MessageHandler(hwnd, message, wparam, lparam);
}

void FlutterWindow::SessionEnding() {
  if (!instance_) {
    return;
  }
  // Windows ends the process once this returns: the app is asked to sign
  // its phone out, and messages are pumped so the engine can answer.
  auto done = std::make_shared<bool>(false);
  auto finish = [done]() { *done = true; };
  instance_->InvokeMethod(
      "ending", nullptr,
      std::make_unique<flutter::MethodResultFunctions<flutter::EncodableValue>>(
          [finish](const flutter::EncodableValue*) { finish(); },
          [finish](const std::string&, const std::string&,
                   const flutter::EncodableValue*) { finish(); },
          [finish]() { finish(); }));
  ::ShutdownBlockReasonCreate(GetHandle(), L"Signing out of calls");
  const ULONGLONG until = ::GetTickCount64() + 4000;
  MSG msg;
  while (!*done && ::GetTickCount64() < until) {
    if (::PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) {
      if (msg.message == WM_QUIT) {
        break;
      }
      ::TranslateMessage(&msg);
      ::DispatchMessageW(&msg);
    } else {
      ::MsgWaitForMultipleObjects(0, nullptr, FALSE, 50, QS_ALLINPUT);
    }
  }
  ::ShutdownBlockReasonDestroy(GetHandle());
}
