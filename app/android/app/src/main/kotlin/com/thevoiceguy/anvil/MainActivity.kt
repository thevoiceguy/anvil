package com.thevoiceguy.anvil

import android.Manifest
import android.app.PictureInPictureParams
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.os.Build
import android.os.Bundle
import android.util.Rational
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    private var microphoneAnswer: MethodChannel.Result? = null
    private var cameraAnswer: MethodChannel.Result? = null
    private var screenAnswer: MethodChannel.Result? = null
    private var network: NetworkWatch? = null

    /** Picture-in-picture as allowed now (a call shows their picture). */
    private var pip: PictureInPictureParams? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        // Before Dart opens the library: its audio needs the JVM. The
        // phone's calls reach Telecom from here on, Dart or not.
        PhoneLink.start(this)
        ScreenCapture.register(this)
        super.onCreate(savedInstanceState)
        // Where FCP wakes the app: the token, when built with Firebase.
        Push.register(this)
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
        VideoTextures.register(flutterEngine)
        val channel = MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "anvil/mobile")
        CallSystem.channel = channel
        network = NetworkWatch(this, channel)
        channel.setMethodCallHandler { call, result ->
            when (call.method) {
                "requestMicrophone" -> requestMicrophone(result)
                "requestCamera" -> requestCamera(result)
                "requestScreenCapture" -> requestScreenCapture(result)
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
                "cellular" -> result.success(network?.cellular())
                "setPictureInPicture" -> {
                    setPictureInPicture(call)
                    result.success(null)
                }
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
                // A push as FCM delivers one, for the app's tests on a
                // device with no Firebase project; debug builds only.
                "deliverPush" -> {
                    if (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE == 0) {
                        result.notImplemented()
                    } else {
                        val data = call.arguments<Map<String, String>>() ?: emptyMap()
                        Push.handle(this, data)
                        result.success(null)
                    }
                }
                else -> result.notImplemented()
            }
        }
    }

    override fun cleanUpFlutterEngine(flutterEngine: FlutterEngine) {
        CallSystem.channel = null
        VideoTextures.clear()
        network?.stop()
        network = null
        super.cleanUpFlutterEngine(flutterEngine)
    }

    /**
     * Picture-in-picture allowed with the picture's shape, or not: from
     * Android 12 the window shrinks by itself when the user leaves the app;
     * before, when they leave it ([onUserLeaveHint]).
     */
    private fun setPictureInPicture(call: MethodCall) {
        if (!packageManager.hasSystemFeature(PackageManager.FEATURE_PICTURE_IN_PICTURE)) return
        val enabled = call.argument<Boolean>("enabled") == true
        val width = (call.argument<Number>("width") ?: 16).toInt().coerceAtLeast(1)
        val height = (call.argument<Number>("height") ?: 9).toInt().coerceAtLeast(1)
        // Android takes shapes from 1:2.39 to 2.39:1.
        val ratio = (width.toDouble() / height).coerceIn(1 / 2.39 + 0.001, 2.39 - 0.001)
        val builder = PictureInPictureParams.Builder()
            .setAspectRatio(Rational((ratio * 1000).toInt(), 1000))
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setAutoEnterEnabled(enabled).setSeamlessResizeEnabled(true)
        }
        val params = builder.build()
        setPictureInPictureParams(params)
        pip = if (enabled) params else null
    }

    override fun onUserLeaveHint() {
        super.onUserLeaveHint()
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            pip?.let { enterPictureInPictureMode(it) }
        }
    }

    override fun onPictureInPictureModeChanged(inPictureInPicture: Boolean, newConfig: Configuration) {
        super.onPictureInPictureModeChanged(inPictureInPicture, newConfig)
        CallSystem.channel?.invokeMethod("pictureInPicture", mapOf("on" to inPictureInPicture))
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

    /** Before our camera is first used in a call; whether the app may use it. */
    private fun requestCamera(result: MethodChannel.Result) {
        if (checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            result.success(true)
            return
        }
        cameraAnswer?.success(false)
        cameraAnswer = result
        requestPermissions(arrayOf(Manifest.permission.CAMERA), CAMERA)
    }

    /** Before each share of the screen: Android asks the user every time. */
    private fun requestScreenCapture(result: MethodChannel.Result) {
        screenAnswer?.success(false)
        screenAnswer = result
        try {
            @Suppress("DEPRECATION")
            startActivityForResult(ScreenCapture.consentIntent(this), SCREEN)
        } catch (e: Exception) {
            screenAnswer = null
            result.success(false)
        }
    }

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        @Suppress("DEPRECATION")
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != SCREEN) return
        val answer = screenAnswer ?: return
        screenAnswer = null
        if (resultCode != RESULT_OK || data == null) {
            answer.success(false)
            return
        }
        // Answered once the service holds the projection.
        ScreenCapture.allowed(resultCode, data) { ok -> runOnUiThread { answer.success(ok) } }
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
        if (requestCode == CAMERA) {
            val granted = checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
            cameraAnswer?.success(granted)
            cameraAnswer = null
        }
    }

    private companion object {
        const val MICROPHONE = 1
        const val CAMERA = 2
        const val SCREEN = 3
    }
}
