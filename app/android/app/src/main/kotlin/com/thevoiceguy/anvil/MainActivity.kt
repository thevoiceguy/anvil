package com.thevoiceguy.anvil

import android.Manifest
import android.content.pm.PackageManager
import android.os.Bundle
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    private var microphoneAnswer: MethodChannel.Result? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        // Before Dart opens the library: its audio needs the JVM.
        AnvilNative.init(applicationContext)
        super.onCreate(savedInstanceState)
    }

    // `anvil/mobile`: what the app asks of the phone (lib/src/mobile).
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "anvil/mobile")
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "requestMicrophone" -> requestMicrophone(result)
                    // Android's AAudio needs nothing set up front; the
                    // per-call audio mode comes with U4b.
                    "startAudio" -> result.success(null)
                    else -> result.notImplemented()
                }
            }
    }

    private fun requestMicrophone(result: MethodChannel.Result) {
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED
        ) {
            result.success(true)
            return
        }
        // One question at a time; a second ask waits on the same answer.
        microphoneAnswer?.success(false)
        microphoneAnswer = result
        requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), MICROPHONE)
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == MICROPHONE) {
            val granted = grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED
            microphoneAnswer?.success(granted)
            microphoneAnswer = null
        }
    }

    private companion object {
        const val MICROPHONE = 1
    }
}
