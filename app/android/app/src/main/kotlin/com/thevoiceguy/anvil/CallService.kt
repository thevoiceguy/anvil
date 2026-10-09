package com.thevoiceguy.anvil

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.RingtoneManager
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.Person
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat

/**
 * Runs while the app has a call: a foreground service, so the call keeps
 * its microphone when the user leaves the app, and its notification is the
 * call's screen outside the app — ringing, with Answer and Decline, for a
 * call coming in (and on the lock screen); with Hang up once it is up.
 */
class CallService : Service() {
    companion object {
        private const val TAG = "AnvilCalls"
        private const val NOTIFICATION = 7
        private const val RINGING = "calls_ringing"
        private const val ONGOING = "calls_ongoing"
        const val ANSWER = "com.thevoiceguy.anvil.ANSWER"
        const val END = "com.thevoiceguy.anvil.END"
        const val EXTRA_CALL = "call"

        var appContext: Context? = null
        private var running = false

        /** Start, refresh or stop the service for the calls there are now. */
        fun update(context: Context) {
            appContext = context.applicationContext
            val app = context.applicationContext
            CallSystem.onCalls?.invoke(CallSystem.calls.isNotEmpty())
            if (CallSystem.calls.isEmpty()) {
                if (running) app.stopService(Intent(app, CallService::class.java))
                NotificationManagerCompat.from(app).cancel(NOTIFICATION)
                return
            }
            try {
                ContextCompat.startForegroundService(app, Intent(app, CallService::class.java))
            } catch (e: Exception) {
                // Started from the background, which Android may refuse: the
                // notification alone still rings and answers.
                Log.w(TAG, "the call's service did not start", e)
                post(app)
            }
        }

        private fun channels(context: Context) {
            val manager = context.getSystemService(NotificationManager::class.java)
            val ringing = NotificationChannel(
                RINGING,
                context.getString(R.string.calls_ringing_channel),
                NotificationManager.IMPORTANCE_HIGH,
            ).apply {
                setSound(
                    RingtoneManager.getDefaultUri(RingtoneManager.TYPE_RINGTONE),
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                        .build(),
                )
                enableVibration(true)
            }
            val ongoing = NotificationChannel(
                ONGOING,
                context.getString(R.string.calls_ongoing_channel),
                NotificationManager.IMPORTANCE_LOW,
            )
            manager.createNotificationChannel(ringing)
            manager.createNotificationChannel(ongoing)
        }

        private fun pending(context: Context, action: String, call: Long, activity: Boolean): PendingIntent {
            val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
            val code = (call * 4 + if (action == ANSWER) 1 else 2).toInt()
            return if (activity) {
                val intent = Intent(context, MainActivity::class.java)
                    .setAction(action)
                    .putExtra(EXTRA_CALL, call)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
                PendingIntent.getActivity(context, code, intent, flags)
            } else {
                val intent = Intent(context, CallActionReceiver::class.java)
                    .setAction(action)
                    .putExtra(EXTRA_CALL, call)
                PendingIntent.getBroadcast(context, code, intent, flags)
            }
        }

        /** The notification for the calls there are: ringing first. */
        fun notification(context: Context): Notification? {
            channels(context)
            val ringing = CallSystem.calls.values.firstOrNull { it.incoming && !it.connected }
            val call = ringing ?: CallSystem.calls.values.lastOrNull() ?: return null
            val person = Person.Builder().setName(call.name.ifEmpty { call.number }).setImportant(true).build()
            val open = PendingIntent.getActivity(
                context,
                0,
                Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            val builder = NotificationCompat.Builder(context, if (ringing != null) RINGING else ONGOING)
                .setSmallIcon(R.mipmap.ic_launcher)
                .setCategory(NotificationCompat.CATEGORY_CALL)
                .setOngoing(true)
                .setContentIntent(open)
                .setContentText(call.number)
            if (ringing != null) {
                builder
                    .setPriority(NotificationCompat.PRIORITY_MAX)
                    .setFullScreenIntent(open, true)
                    .setStyle(
                        NotificationCompat.CallStyle.forIncomingCall(
                            person,
                            pending(context, END, call.id, activity = false),
                            pending(context, ANSWER, call.id, activity = true),
                        ),
                    )
            } else {
                builder.setStyle(
                    NotificationCompat.CallStyle.forOngoingCall(
                        person,
                        pending(context, END, call.id, activity = false),
                    ),
                )
            }
            val notification = builder.build()
            // A call ringing in rings until it is answered or gone.
            if (ringing != null) notification.flags = notification.flags or Notification.FLAG_INSISTENT
            return notification
        }

        @Suppress("MissingPermission")
        private fun post(context: Context) {
            val manager = NotificationManagerCompat.from(context)
            if (!manager.areNotificationsEnabled()) return
            notification(context)?.let { manager.notify(NOTIFICATION, it) }
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val notification = notification(this)
        if (notification == null) {
            stopSelf()
            return START_NOT_STICKY
        }
        var types = 0
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            types = ServiceInfo.FOREGROUND_SERVICE_TYPE_PHONE_CALL
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R &&
            checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        ) {
            types = types or ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
        }
        try {
            ServiceCompat.startForeground(this, NOTIFICATION, notification, types)
            running = true
        } catch (e: Exception) {
            Log.w(TAG, "the call's service could not come forward", e)
            post(this)
            stopSelf()
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        running = false
        super.onDestroy()
    }
}
