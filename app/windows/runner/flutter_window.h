#ifndef RUNNER_FLUTTER_WINDOW_H_
#define RUNNER_FLUTTER_WINDOW_H_

#include <flutter/dart_project.h>
#include <flutter/encodable_value.h>
#include <flutter/flutter_view_controller.h>
#include <flutter/method_channel.h>

#include <memory>

#include "win32_window.h"

// A window that does nothing but host a Flutter view.
class FlutterWindow : public Win32Window {
 public:
  // Creates a new FlutterWindow hosting a Flutter view running |project|;
  // |hidden| keeps it unshown until the app shows it (start at login).
  FlutterWindow(const flutter::DartProject& project, bool hidden);
  virtual ~FlutterWindow();

 protected:
  // Win32Window:
  bool OnCreate() override;
  void OnDestroy() override;
  LRESULT MessageHandler(HWND window, UINT const message, WPARAM const wparam,
                         LPARAM const lparam) noexcept override;

 private:
  // The project to run.
  flutter::DartProject project_;

  // The Flutter instance hosted by this window.
  std::unique_ptr<flutter::FlutterViewController> flutter_controller_;

  // Starts handed on by a second start, and the session ending
  // (lib/src/desktop/native_shell.dart).
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> instance_;

  bool hidden_;

  // The session is ending: the app signs its phone out first, for up to
  // four seconds.
  void SessionEnding();
};

#endif  // RUNNER_FLUTTER_WINDOW_H_
