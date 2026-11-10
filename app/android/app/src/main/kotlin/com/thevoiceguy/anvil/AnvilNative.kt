package com.thevoiceguy.anvil

import android.content.Context
import android.view.Surface

/** The Rust core, loaded by the app so it knows the JVM and the context. */
object AnvilNative {
    init {
        System.loadLibrary("anvil_bridge")
    }

    /** Hands the JVM and the application context to the Rust core. */
    @JvmStatic
    external fun init(context: Context)

    // What PhoneLink asks of the phone (app/rust/src/android.rs). Each
    // blocks until done and answers null, or what went wrong.

    /** A push came: start the phone from the saved session, or register again. */
    @JvmStatic
    external fun wake(sessionPath: String): String?

    /** Where this install is woken; an empty token is nowhere. */
    @JvmStatic
    external fun setPush(sessionPath: String, provider: String, param: String, token: String): String?

    /** The next event for the system's call screen as JSON; null when none came in time. */
    @JvmStatic
    external fun nextCallEvent(timeoutMs: Long): String?

    /** What the user did at the system's call screen, as JSON. */
    @JvmStatic
    external fun act(json: String): String?

    // Video tiles' surfaces (app/rust/src/android_video.rs; VideoTextures).

    /** A slot for a new tile's surface. */
    @JvmStatic
    external fun videoSlot(): Long

    /** The tile's surface now, or none while Flutter has let it go. */
    @JvmStatic
    external fun videoSurface(slot: Long, surface: Surface?)

    /** The tile is gone: Dart detached it from the phone first. */
    @JvmStatic
    external fun videoRelease(slot: Long)

    /** The address of the function that draws a picture on a slot's surface. */
    @JvmStatic
    external fun videoPresent(): Long
}
