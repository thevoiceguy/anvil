package com.thevoiceguy.anvil

import android.content.Context

/** The Rust core, loaded by the app so it knows the JVM and the context. */
object AnvilNative {
    init {
        System.loadLibrary("anvil_bridge")
    }

    /** Hands the JVM and the application context to the Rust core. */
    @JvmStatic
    external fun init(context: Context)
}
