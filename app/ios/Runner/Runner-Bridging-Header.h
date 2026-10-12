#import "GeneratedPluginRegistrant.h"

#include <stdint.h>

// The phone in the Rust core, for native code when Dart may not be running
// (app/rust/src/ios.rs). Each answers NULL, or what went wrong as a string
// freed with anvil_string_free.
char *anvil_wake(const char *session_path);
char *anvil_set_push(const char *session_path, const char *provider, const char *param,
                     const char *token);
/// The next event for CallKit as JSON; NULL when none came in time.
char *anvil_next_call_event(int64_t timeout_ms);
char *anvil_act(const char *json);
void anvil_string_free(char *s);
