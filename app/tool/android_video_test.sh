#!/bin/sh
# anvil-video's tests on a running emulator (CI's Android job): the
# phone's MediaCodec and cameras, run as a bare program from adb's shell.
# CI builds the program for the emulator's ABI into anvil-video-tests.
set -eu
adb push anvil-video-tests /data/local/tmp/anvil-video-tests >/dev/null
adb shell chmod 755 /data/local/tmp/anvil-video-tests
# adb's shell does not pass the program's status back on every version,
# so the result is read from what the test harness prints.
out=$(adb shell "cd /data/local/tmp && ./anvil-video-tests --test-threads=1 2>&1; echo exit=\$?")
echo "$out"
echo "$out" | grep -q '^exit=0' && echo "$out" | grep -q 'test result: ok'
