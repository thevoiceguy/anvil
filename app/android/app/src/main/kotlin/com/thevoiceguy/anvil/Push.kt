package com.thevoiceguy.anvil

import android.annotation.SuppressLint
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import com.google.firebase.FirebaseApp
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage

/**
 * FCP wakes the app through Firebase Cloud Messaging (FCP's `docs/PUSH.md`):
 * the app registers its FCM token as its push address, and FCP pushes a
 * call to it when the app is asleep. A call push rings at once — Telecom and
 * the call's notification — while the phone starts and registers, and the
 * call's INVITE then takes the ringing call's place ([CallSystem]).
 *
 * The Firebase project is the app publisher's, given at build time
 * (`ANVIL_FCM_*`, app/android/app/build.gradle.kts); an app built without
 * one has no token and registers without a push address.
 */
object Push {
    private const val TAG = "AnvilPush"
    private const val NOTICES = "push_notices"
    private const val TEST_NOTICE = 8
    private val main = Handler(Looper.getMainLooper())

    /** The Firebase project this build pushes through, if any. */
    private fun project(context: Context): String? =
        FirebaseApp.getApps(context).firstOrNull()?.options?.projectId

    /** Ask Firebase for this install's token, and register it. */
    fun register(context: Context) {
        val app = context.applicationContext
        val project = project(app) ?: return
        FirebaseMessaging.getInstance().token.addOnCompleteListener { task ->
            if (task.isSuccessful) {
                PhoneLink.setPush(app, "fcm", project, task.result)
            } else {
                // No Google Play services, or no network yet: the app
                // registers without a push address, and is asked again at
                // its next start.
                Log.w(TAG, "no FCM token", task.exception)
            }
        }
    }

    /** Firebase renewed the token. */
    fun tokenChanged(context: Context, token: String) {
        val project = project(context) ?: return
        PhoneLink.setPush(context, "fcm", project, token)
    }

    /** A push's data (`type`, `call`, `from`, `name`, `tenant`, `sent`). */
    fun handle(context: Context, data: Map<String, String>) {
        val app = context.applicationContext
        val call = data["call"].orEmpty()
        when (data["type"]) {
            "call" -> main.post {
                // Signed out since the token was registered: nothing rings.
                if (!PhoneLink.signedIn(app)) {
                    Log.i(TAG, "a call push for an app signed out")
                    return@post
                }
                PhoneLink.start(app)
                CallSystem.reportPushed(app, call, data["name"].orEmpty(), data["from"].orEmpty())
                PhoneLink.wake(app) { CallSystem.wakeFailed(app, call) }
            }
            "cancel" -> main.post { CallSystem.cancelPushed(app, call) }
            "test" -> testNotice(app)
            else -> Log.i(TAG, "a push of a kind this app does not know: ${data["type"]}")
        }
    }

    /** An administrator's test push: shown, and gone again. */
    @SuppressLint("MissingPermission")
    private fun testNotice(context: Context) {
        val manager = NotificationManagerCompat.from(context)
        if (!manager.areNotificationsEnabled()) return
        context.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(
                NOTICES,
                context.getString(R.string.push_notices_channel),
                NotificationManager.IMPORTANCE_DEFAULT,
            ),
        )
        val notice = NotificationCompat.Builder(context, NOTICES)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle(context.getString(R.string.push_test_title))
            .setContentText(context.getString(R.string.push_test_text))
            .setAutoCancel(true)
            .setTimeoutAfter(10_000)
            .build()
        manager.notify(TEST_NOTICE, notice)
    }
}

/** Where FCM delivers the app's pushes and its new tokens. */
class AnvilMessagingService : FirebaseMessagingService() {
    override fun onMessageReceived(message: RemoteMessage) = Push.handle(this, message.data)

    override fun onNewToken(token: String) = Push.tokenChanged(applicationContext, token)
}
