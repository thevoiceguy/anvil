#include "instance.h"

#include <cwchar>

namespace single_instance {
namespace {

// Per session ("Local\"), so another user's Anvil on the same machine is
// theirs; held until the process ends.
constexpr wchar_t kMutexName[] = L"Local\\Anvil.Softphone";
constexpr wchar_t kWindowClass[] = L"FLUTTER_RUNNER_WIN32_WINDOW";

BOOL CALLBACK FindApp(HWND window, LPARAM found) {
  wchar_t name[64];
  if (::GetClassNameW(window, name, 64) && wcscmp(name, kWindowClass) == 0 &&
      ::GetPropW(window, kWindowProp)) {
    *reinterpret_cast<HWND*>(found) = window;
    return FALSE;
  }
  return TRUE;
}

}  // namespace

bool Claim(const std::vector<std::string>& args) {
  ::CreateMutexW(nullptr, FALSE, kMutexName);
  if (::GetLastError() != ERROR_ALREADY_EXISTS) {
    return true;
  }
  // The running app may still be making its window: five seconds.
  HWND app = nullptr;
  for (int i = 0; i < 50 && !app; i++) {
    ::EnumWindows(FindApp, reinterpret_cast<LPARAM>(&app));
    if (!app) ::Sleep(100);
  }
  if (!app) {
    return false;
  }
  // The arguments as UTF-8, each ended by a NUL.
  std::string joined;
  for (const auto& arg : args) {
    joined.append(arg);
    joined.push_back('\0');
  }
  COPYDATASTRUCT data{};
  data.dwData = kForwarded;
  data.cbData = static_cast<DWORD>(joined.size());
  data.lpData = joined.data();
  // This start has the foreground; the running app may take it.
  DWORD pid = 0;
  ::GetWindowThreadProcessId(app, &pid);
  ::AllowSetForegroundWindow(pid);
  ::SendMessageTimeoutW(app, WM_COPYDATA, 0, reinterpret_cast<LPARAM>(&data),
                        SMTO_ABORTIFHUNG, 5000, nullptr);
  return false;
}

std::vector<std::string> Arguments(const COPYDATASTRUCT* data) {
  std::vector<std::string> args;
  if (data == nullptr || data->dwData != kForwarded || data->lpData == nullptr) {
    return args;
  }
  const char* bytes = static_cast<const char*>(data->lpData);
  std::string arg;
  for (DWORD i = 0; i < data->cbData; i++) {
    if (bytes[i] == '\0') {
      args.push_back(arg);
      arg.clear();
    } else {
      arg.push_back(bytes[i]);
    }
  }
  return args;
}

}  // namespace single_instance
