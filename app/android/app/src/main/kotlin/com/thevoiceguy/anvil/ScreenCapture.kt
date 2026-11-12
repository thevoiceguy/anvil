package com.thevoiceguy.anvil

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.DisplayMetrics
import android.util.Log
import android.view.Display
import android.view.Surface
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat

/**
 * The phone's screen, shared in a room (FCP's `docs/SOFTPHONE_VIDEO.md`,
 * A5). Android asks the user before each share (the consent dialog
 * `MainActivity` shows); the projection given is held by
 * [ScreenCaptureService], a foreground service of type `mediaProjection`,
 * as Android 14 requires before the projection is taken. Rust asks for the
 * picture (app/rust/src/android_screen.rs): [start] draws the projection
 * onto the surface of Rust's image reader through a virtual display. A
 * projection draws one display only, so each share asks again.
 *
 * Rust calls [display], [start] and [stop] on the object from its own
 * thread; they are instance methods so it finds them through the object.
 */
object ScreenCapture {
    private const val TAG = "AnvilScreen"

    private lateinit var app: Context
    private val main = Handler(Looper.getMainLooper())

    private var projection: MediaProjection? = null
    private var display: VirtualDisplay? = null

    /** The user's consent, until the service has come forward to take it. */
    private var consent: Pair<Int, Intent>? = null
    private var answer: ((Boolean) -> Unit)? = null

    /** At start: Rust may ask for the screen from now on. */
    fun register(context: Context) {
        app = context.applicationContext
        AnvilNative.screenCapture(this)
    }

    /** What `MainActivity` starts to ask the user. */
    fun consentIntent(context: Context): Intent =
        context.getSystemService(MediaProjectionManager::class.java).createScreenCaptureIntent()

    /** The user allowed it: the service comes forward and takes it. */
    fun allowed(code: Int, data: Intent, then: (Boolean) -> Unit) {
        synchronized(this) {
            answer?.invoke(false)
            consent = code to data
            answer = then
        }
        try {
            ContextCompat.startForegroundService(app, Intent(app, ScreenCaptureService::class.java))
        } catch (e: Exception) {
            Log.w(TAG, "the share's service would not start", e)
            take(null)
        }
    }

    /** From the service, once in the foreground (or not, `null`). */
    internal fun take(service: Service?) {
        val (given, then) = synchronized(this) {
            val c = consent to answer
            consent = null
            answer = null
            c
        }
        var ok = false
        if (service != null && given != null) {
            try {
                val manager = app.getSystemService(MediaProjectionManager::class.java)
                val p = manager.getMediaProjection(given.first, given.second)
                if (p != null) {
                    // Before its display, as Android 14 requires.
                    p.registerCallback(ended, main)
                    synchronized(this) {
                        projection?.stop()
                        projection = p
                    }
                    ok = true
                }
            } catch (e: Exception) {
                Log.w(TAG, "no projection", e)
            }
        }
        if (!ok) stopService()
        then?.invoke(ok)
    }

    private val ended = object : MediaProjection.Callback() {
        override fun onStop() {
            synchronized(this@ScreenCapture) {
                display?.release()
                display = null
                projection = null
            }
            stopService()
            AnvilNative.screenCaptureEnded()
        }
    }

    private fun stopService() {
        app.stopService(Intent(app, ScreenCaptureService::class.java))
    }

    /** The display's width, height (pixels) and density, for Rust. */
    fun display(): IntArray {
        val d = app.getSystemService(DisplayManager::class.java).getDisplay(Display.DEFAULT_DISPLAY)
        val m = DisplayMetrics()
        @Suppress("DEPRECATION")
        d.getRealMetrics(m)
        return intArrayOf(m.widthPixels, m.heightPixels, m.densityDpi)
    }

    /** Draw the projection on [surface]; null, or why not. */
    fun start(surface: Surface, width: Int, height: Int, dpi: Int): String? = synchronized(this) {
        val p = projection ?: return "Android asks before each share: press Share again"
        if (display != null) return "the screen is shared already"
        return try {
            display = p.createVirtualDisplay(
                "Anvil share",
                width,
                height,
                dpi,
                DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR,
                surface,
                null,
                main,
            )
            null
        } catch (e: Exception) {
            Log.w(TAG, "no virtual display", e)
            "the screen would not share: ${e.message}"
        }
    }

    /** The share is over: the display and the projection go. */
    fun stop() {
        val p = synchronized(this) {
            display?.release()
            display = null
            val p = projection
            projection = null
            p
        }
        // Its callback stops the service.
        if (p != null) main.post { p.stop() } else stopService()
    }
}

/** Holds the projection in the foreground while the screen is shared. */
class ScreenCaptureService : Service() {
    companion object {
        private const val NOTIFICATION = 8
        private const val CHANNEL = "screen_share"
        private const val STOP = "com.thevoiceguy.anvil.STOP_SHARE"
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == STOP) {
            ScreenCapture.stop()
            return START_NOT_STICKY
        }
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL,
                getString(R.string.screen_share_channel),
                NotificationManager.IMPORTANCE_LOW,
            ),
        )
        val stop = PendingIntent.getService(
            this,
            0,
            Intent(this, ScreenCaptureService::class.java).setAction(STOP),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle(getString(R.string.screen_share_title))
            .setOngoing(true)
            .setSilent(true)
            .addAction(0, getString(R.string.screen_share_stop), stop)
            .build()
        val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION
        } else {
            0
        }
        try {
            ServiceCompat.startForeground(this, NOTIFICATION, notification, type)
            ScreenCapture.take(this)
        } catch (e: Exception) {
            Log.w("AnvilScreen", "the share's service could not come forward", e)
            ScreenCapture.take(null)
            stopSelf()
        }
        return START_NOT_STICKY
    }
}
