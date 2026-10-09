#!/bin/sh
# The mobile integration test on a running emulator (CI's Android job), with
# the device's log kept: a crash at launch leaves `flutter test` waiting
# with nothing to say, and the reason only in logcat.
set -u
adb logcat -c
adb logcat -v time > logcat.txt 2>&1 &
logcat=$!
timeout 900 flutter test integration_test/mobile_test.dart -d emulator-5554
status=$?
kill $logcat 2>/dev/null
if [ $status -ne 0 ]; then
  echo "--- logcat: the app, the runtime, crashes ---"
  grep -E "AndroidRuntime|FATAL|DEBUG|libc|anvil|flutter|Rust|cpal|oboe|AAudio|linker|UnsatisfiedLink" logcat.txt | tail -300
fi
exit $status
