package com.thevoiceguy.anvil

import android.annotation.SuppressLint
import android.content.ComponentName
import android.content.Context
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.OutcomeReceiver
import android.os.PowerManager
import android.telecom.CallAudioState
import android.telecom.CallEndpoint
import android.telecom.CallEndpointException
import android.telecom.DisconnectCause
import android.telecom.PhoneAccount
import android.telecom.PhoneAccountHandle
import android.telecom.TelecomManager
import android.util.Log
import io.flutter.plugin.common.MethodChannel

/**
 * The app's calls as Android's Telecom knows them (a self-managed
 * ConnectionService): the app reports each call, Telecom keeps the phone's
 * audio and other calling apps in step, and what the user does at the
 * call's notification or a headset comes back as `callEvent`s on
 * `anvil/mobile` (lib/src/mobile/platform.dart).
 */
object CallSystem {
    private const val TAG = "AnvilCalls"
    private val main = Handler(Looper.getMainLooper())

    /** The channel to Dart while the app's engine runs. */
    var channel: MethodChannel? = null

    /** A call: who it is with, and where it is. */
    class Call(val id: Long, val name: String, val number: String, val incoming: Boolean) {
        var connected = false
        var held = false
        var connection: AnvilConnection? = null
    }

    val calls = LinkedHashMap<Long, Call>()

    /** Told whenever the calls change: whether there are any. */
    var onCalls: ((Boolean) -> Unit)? = null

    private fun handle(context: Context) =
        PhoneAccountHandle(ComponentName(context, AnvilConnectionService::class.java), "anvil")

    private fun telecom(context: Context) =
        context.getSystemService(Context.TELECOM_SERVICE) as TelecomManager

    private var registered = false

    /** The app's account with Telecom: its own calls, which it shows itself. */
    private fun register(context: Context) {
        if (registered) return
        val account = PhoneAccount.builder(handle(context), context.getString(R.string.app_name))
            .setCapabilities(PhoneAccount.CAPABILITY_SELF_MANAGED)
            .setSupportedUriSchemes(listOf("sip", PhoneAccount.SCHEME_TEL))
            .build()
        telecom(context).registerPhoneAccount(account)
        registered = true
    }

    // Where a call's audio goes: Telecom routes a self-managed call's audio
    // and tells each connection the routes there are (as call endpoints
    // from Android 14, as a CallAudioState before). Kept for Dart as
    // `{current, available, bluetoothName}` (lib/src/mobile/platform.dart).

    private var endpoints: List<CallEndpoint> = emptyList()
    private var routes: Map<String, Any?>? = null

    /** What Dart asks for: the routes now, or null with no call. */
    fun audioRoutes(): Map<String, Any?>? = if (calls.isEmpty()) null else routes

    private fun publish(routes: Map<String, Any?>) {
        if (routes == this.routes) return
        this.routes = routes
        main.post { channel?.invokeMethod("audioRoutes", routes) }
    }

    private fun endpointRoute(type: Int): String? = when (type) {
        CallEndpoint.TYPE_EARPIECE -> "earpiece"
        CallEndpoint.TYPE_SPEAKER -> "speaker"
        CallEndpoint.TYPE_BLUETOOTH -> "bluetooth"
        CallEndpoint.TYPE_WIRED_HEADSET -> "wired"
        else -> null
    }

    /** Android 14 and later: the endpoint in use, and the ones there are. */
    fun endpointsChanged(current: CallEndpoint?, available: List<CallEndpoint>?) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) return
        if (available != null) endpoints = available
        val now = current?.let { endpointRoute(it.endpointType) }
            ?: (routes?.get("current") as? String)
            ?: return
        val bluetooth = (current?.takeIf { it.endpointType == CallEndpoint.TYPE_BLUETOOTH }
            ?: endpoints.firstOrNull { it.endpointType == CallEndpoint.TYPE_BLUETOOTH })
        publish(
            mapOf(
                "current" to now,
                "available" to endpoints.mapNotNull { endpointRoute(it.endpointType) }.distinct(),
                "bluetoothName" to bluetooth?.endpointName?.toString(),
            ),
        )
    }

    /** Before Android 14: the route in use, and a mask of the ones there are. */
    @Suppress("DEPRECATION")
    fun audioStateChanged(state: CallAudioState) {
        val names = listOf(
            CallAudioState.ROUTE_EARPIECE to "earpiece",
            CallAudioState.ROUTE_SPEAKER to "speaker",
            CallAudioState.ROUTE_BLUETOOTH to "bluetooth",
            CallAudioState.ROUTE_WIRED_HEADSET to "wired",
        )
        val current = names.firstOrNull { it.first == state.route }?.second ?: return
        // A Bluetooth device's name needs BLUETOOTH_CONNECT, which the app
        // does not ask for: "Bluetooth" stands in.
        publish(
            mapOf(
                "current" to current,
                "available" to names.filter { state.supportedRouteMask and it.first != 0 }.map { it.second },
                "bluetoothName" to null,
            ),
        )
    }

    /** Send the call's audio this way: through any of the app's connections. */
    @Suppress("DEPRECATION")
    fun setAudioRoute(route: String) {
        val connection = calls.values.firstNotNullOfOrNull { it.connection } ?: return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            val endpoint = endpoints.firstOrNull { endpointRoute(it.endpointType) == route } ?: return
            connection.requestCallEndpointChange(
                endpoint,
                { it.run() },
                object : OutcomeReceiver<Void, CallEndpointException> {
                    override fun onResult(result: Void?) {}

                    override fun onError(error: CallEndpointException) {
                        Log.w(TAG, "Telecom did not change the route", error)
                    }
                },
            )
        } else {
            val mask = when (route) {
                "earpiece" -> CallAudioState.ROUTE_EARPIECE
                "speaker" -> CallAudioState.ROUTE_SPEAKER
                "bluetooth" -> CallAudioState.ROUTE_BLUETOOTH
                "wired" -> CallAudioState.ROUTE_WIRED_HEADSET
                else -> return
            }
            connection.setAudioRoute(mask)
        }
    }

    // The screen goes off at the ear (Dart decides when: a call up on the
    // earpiece).
    private var proximity: PowerManager.WakeLock? = null

    // Held for the length of a call, released when Dart says it is over,
    // not within this method (lint's Wakelock rule wants the latter).
    @SuppressLint("Wakelock")
    fun setProximity(context: Context, on: Boolean) {
        val power = context.getSystemService(Context.POWER_SERVICE) as PowerManager
        if (!power.isWakeLockLevelSupported(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK)) return
        val lock = proximity ?: power.newWakeLock(
            PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK,
            "anvil:call",
        ).also {
            it.setReferenceCounted(false)
            proximity = it
        }
        if (on && !lock.isHeld) {
            // A call longer than this keeps going; the sensor stops at the
            // ear. Only so a lock lost track of cannot hold forever.
            lock.acquire(4 * 60 * 60 * 1000L)
        } else if (!on && lock.isHeld) {
            // Off only once the phone has left the ear.
            lock.release(PowerManager.RELEASE_FLAG_WAIT_FOR_NO_PROXIMITY)
        }
    }

    fun emit(action: String, id: Long, extra: Map<String, Any?> = emptyMap()) {
        val event = HashMap<String, Any?>(extra)
        event["action"] = action
        event["id"] = id
        main.post { channel?.invokeMethod("callEvent", event) }
    }

    private fun extras(call: Call) = Bundle().apply {
        putLong("id", call.id)
        putString("name", call.name)
        putString("number", call.number)
    }

    fun reportIncoming(context: Context, id: Long, name: String, number: String) {
        val call = Call(id, name, number, incoming = true)
        calls[id] = call
        try {
            register(context)
            val telecom = telecom(context)
            if (!telecom.isIncomingCallPermitted(handle(context))) {
                // Another app's call (an emergency call) does not let it ring.
                calls.remove(id)
                emit("failed", id, mapOf("reason" to "busy"))
                return
            }
            val bundle = Bundle().apply {
                putBundle(TelecomManager.EXTRA_INCOMING_CALL_EXTRAS, extras(call))
            }
            telecom.addNewIncomingCall(handle(context), bundle)
        } catch (e: Exception) {
            Log.w(TAG, "Telecom refused the incoming call", e)
            CallService.update(context)
            emit("failed", id, mapOf("reason" to "unavailable"))
        }
    }

    // placeCall on the app's own self-managed account needs MANAGE_OWN_CALLS
    // (in the manifest), not CALL_PHONE, which lint assumes.
    @SuppressLint("MissingPermission")
    fun reportOutgoing(context: Context, id: Long, name: String, number: String) {
        val call = Call(id, name, number, incoming = false)
        calls[id] = call
        try {
            register(context)
            val bundle = Bundle().apply {
                putParcelable(TelecomManager.EXTRA_PHONE_ACCOUNT_HANDLE, handle(context))
                putBundle(TelecomManager.EXTRA_OUTGOING_CALL_EXTRAS, extras(call))
            }
            telecom(context).placeCall(Uri.fromParts("sip", number, null), bundle)
        } catch (e: Exception) {
            Log.w(TAG, "Telecom refused the outgoing call", e)
            CallService.update(context)
            emit("failed", id, mapOf("reason" to "unavailable"))
        }
    }

    fun reportConnected(context: Context, id: Long) {
        val call = calls[id] ?: return
        call.connected = true
        call.connection?.setActive()
        CallService.update(context)
    }

    fun reportHeld(context: Context, id: Long, on: Boolean) {
        val call = calls[id] ?: return
        call.held = on
        call.connection?.let { if (on) it.setOnHold() else it.setActive() }
        CallService.update(context)
    }

    fun reportEnded(context: Context, id: Long) {
        val call = calls.remove(id) ?: return
        if (calls.isEmpty()) {
            routes = null
            endpoints = emptyList()
        }
        call.connection?.let {
            it.setDisconnected(DisconnectCause(DisconnectCause.LOCAL))
            it.destroy()
        }
        CallService.update(context)
    }

    /** Telecom made the call's connection. */
    fun attached(context: Context, connection: AnvilConnection) {
        val call = calls[connection.callId]
        if (call == null) {
            // Ended while Telecom was still making it.
            connection.setDisconnected(DisconnectCause(DisconnectCause.LOCAL))
            connection.destroy()
            return
        }
        call.connection = connection
        if (call.connected) connection.setActive()
        if (call.held) connection.setOnHold()
        CallService.update(context)
        emit("shown", call.id)
    }

    /** Telecom could not make the call's connection. */
    fun refused(context: Context, id: Long) {
        CallService.update(context)
        emit("failed", id, mapOf("reason" to "unavailable"))
    }
}
