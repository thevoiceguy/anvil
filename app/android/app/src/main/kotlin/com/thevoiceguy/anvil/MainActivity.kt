package com.thevoiceguy.anvil

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    private var microphoneAnswer: MethodChannel.Result? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        // Before Dart opens the library: its audio needs the JVM.
        AnvilNative.init(applicationContext)
        super.onCreate(savedInstanceState)
        // Over the lock screen only while there is a call to answer or end.
        CallSystem.onCalls = { any -> overLockScreen(any) }
        overLockScreen(CallSystem.calls.isNotEmpty())
        answerFrom(intent)
    }

    override fun onDestroy() {
        CallSystem.onCalls = null
        super.onDestroy()
    }

    private fun overLockScreen(on: Boolean) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
            setShowWhenLocked(on)
            setTurnScreenOn(on)
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        answerFrom(intent)
    }

    /** Answer pressed on the call's notification opens the app here. */
    private fun answerFrom(intent: Intent?) {
        if (intent?.action != CallService.ANSWER) return
        val call = intent.getLongExtra(CallService.EXTRA_CALL, -1)
        if (call >= 0) CallSystem.emit("answer", call)
        intent.action = null
    }

    // `anvil/mobile`: what the app asks of the phone (lib/src/mobile).
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        val channel = MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "anvil/mobile")
        CallSystem.channel = channel
        channel.setMethodCallHandler { call, result ->
            when (call.method) {
                "requestMicrophone" -> requestMicrophone(result)
                // Android's AAudio needs nothing set up front: Telecom
                // sets the call's audio mode and routes it.
                "startAudio" -> result.success(null)
                "reportIncoming" -> {
                    CallSystem.reportIncoming(this, call.id(), call.text("name"), call.text("number"))
                    result.success(null)
                }
                "reportOutgoing" -> {
                    CallSystem.reportOutgoing(this, call.id(), call.text("name"), call.text("number"))
                    result.success(null)
                }
                "reportConnected" -> {
                    CallSystem.reportConnected(this, call.id())
                    result.success(null)
                }
                "reportHeld" -> {
                    CallSystem.reportHeld(this, call.id(), call.argument<Boolean>("on") == true)
                    result.success(null)
                }
                "audioRoutes" -> result.success(CallSystem.audioRoutes())
                "setAudioRoute" -> {
                    CallSystem.setAudioRoute(call.text("route"))
                    result.success(null)
                }
                "setProximity" -> {
                    CallSystem.setProximity(this, call.argument<Boolean>("on") == true)
                    result.success(null)
                }
                "reportEnded" -> {
                    CallSystem.reportEnded(this, call.id())
                    result.success(null)
                }
                else -> result.notImplemented()
            }
        }
    }

    override fun cleanUpFlutterEngine(flutterEngine: FlutterEngine) {
        CallSystem.channel = null
        super.cleanUpFlutterEngine(flutterEngine)
    }

    /** Dart's ints arrive as Integer or Long by size. */
    private fun MethodCall.id(): Long = (argument<Number>("id") ?: -1).toLong()

    private fun MethodCall.text(key: String): String = argument<String>(key) ?: ""

    private fun requestMicrophone(result: MethodChannel.Result) {
        // The call's notification (Android 13 on) is asked for alongside.
        val wanted = buildList {
            add(Manifest.permission.RECORD_AUDIO)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                add(Manifest.permission.POST_NOTIFICATIONS)
            }
        }.filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (wanted.isEmpty()) {
            result.success(true)
            return
        }
        // One question at a time; a second ask waits on the same answer.
        microphoneAnswer?.success(false)
        microphoneAnswer = result
        requestPermissions(wanted.toTypedArray(), MICROPHONE)
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == MICROPHONE) {
            val granted =
                checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
            microphoneAnswer?.success(granted)
            microphoneAnswer = null
        }
    }

    private companion object {
        const val MICROPHONE = 1
    }
}
