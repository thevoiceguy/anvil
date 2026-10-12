#ifndef RUNNER_INSTANCE_H_
#define RUNNER_INSTANCE_H_

#include <windows.h>

#include <string>
#include <vector>

// One Anvil per user session: a second start — the Start menu while the app
// sits in the tray, a tel: or sip: link, start at login after a manual start
// — hands its arguments to the running app and leaves, so two phones never
// register with one device's credentials.
namespace single_instance {

// The property that marks the app's window, for a second start to find it.
inline constexpr wchar_t kWindowProp[] = L"Anvil.Softphone.Window";

// WM_COPYDATA's dwData for a start handed on ("ANV1").
inline constexpr ULONG_PTR kForwarded = 0x414E5631;

// True when this process is the first in the session. Otherwise |args|
// (UTF-8) went to the running app, or it could not be found: either way
// this process should leave.
bool Claim(const std::vector<std::string>& args);

// The arguments a second start handed on.
std::vector<std::string> Arguments(const COPYDATASTRUCT* data);

}  // namespace single_instance

#endif  // RUNNER_INSTANCE_H_
