package com.thevoiceguy.anvil

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.util.concurrent.Executors

/**
 * Native code's way to the phone in the Rust core, with or without Dart: a
 * push starts it in a process where Flutter may never run (FCP's
 * `docs/PUSH.md` §2.5). The phone's calls come here as events for Telecom
 * ([CallSystem.fromPhone]); what the user does at Telecom goes back
 * ([act]). The Rust side is `app/rust/src/system.rs`.
 */
object PhoneLink {
    private const val TAG = "AnvilPhone"
    private val main = Handler(Looper.getMainLooper())

    /** The Rust calls block: one at a time, never on the main thread. */
    private val worker = Executors.newSingleThreadExecutor()

    private var following = false

    /** Where the app keeps its session (as Dart's path_provider has it). */
    fun sessionPath(context: Context): String = File(context.filesDir, "session.json").path

    fun signedIn(context: Context): Boolean = File(sessionPath(context)).exists()

    /** Load the core and follow the phone's calls, once per process. */
    @Synchronized
    fun start(context: Context) {
        val app = context.applicationContext
        AnvilNative.init(app)
        CallService.appContext = app
        if (following) return
        following = true
        Thread({
            while (true) {
                val json = AnvilNative.nextCallEvent(60_000) ?: continue
                val event = try {
                    JSONObject(json)
                } catch (e: Exception) {
                    Log.w(TAG, "an unreadable call event: $json", e)
                    continue
                }
                main.post { CallSystem.fromPhone(app, event) }
            }
        }, "anvil-calls").apply { isDaemon = true }.start()
    }

    /**
     * Woken by a push: start the phone from the saved session, or register
     * again if it runs. [failed] hears why it could not, on the main thread.
     */
    fun wake(context: Context, failed: (String) -> Unit) {
        start(context)
        val session = sessionPath(context)
        worker.execute {
            val error = AnvilNative.wake(session) ?: return@execute
            Log.w(TAG, "the phone did not wake: $error")
            main.post { failed(error) }
        }
    }

    /** Where this install is woken from now on; an empty token is nowhere. */
    fun setPush(context: Context, provider: String, param: String, token: String) {
        start(context)
        val session = sessionPath(context)
        worker.execute {
            AnvilNative.setPush(session, provider, param, token)?.let {
                Log.w(TAG, "the push address was not kept: $it")
            }
        }
    }

    /** What the user did at the system's call screen, for the phone. */
    fun act(action: String, id: Long, extra: Map<String, Any?> = emptyMap()) {
        val json = JSONObject(extra).put("action", action).put("id", id).toString()
        worker.execute {
            AnvilNative.act(json)?.let { Log.i(TAG, "$action on call $id: $it") }
        }
    }
}
