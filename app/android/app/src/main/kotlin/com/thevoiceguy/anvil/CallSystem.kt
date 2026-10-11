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
import android.os.SystemClock
import android.util.Log
import io.flutter.plugin.common.MethodChannel
import org.json.JSONObject

/**
 * The app's calls as Android's Telecom knows them (a self-managed
 * ConnectionService): the phone in the Rust core tells of each call
 * ([fromPhone], through [PhoneLink]), Telecom keeps the phone's audio and
 * other calling apps in step, and what the user does at the call's
 * notification or a headset goes back to the phone — and to Dart, when it
 * runs, as `callEvent`s on `anvil/mobile` (lib/src/mobile/platform.dart).
 *
 * A call a push told of (FCP's `docs/PUSH.md` §2.5) rings at once, before
 * the phone has woken and registered; the phone's call, when its INVITE
 * comes, takes its place — the one FCP names as the push did.
 */
object CallSystem {
    private const val TAG = "AnvilCalls"
    private val main = Handler(Looper.getMainLooper())

    /** The channel to Dart while the app's engine runs. */
    var channel: MethodChannel? = null

    /** A call: who it is with, and where it is. */
    class Call(var id: Long, val name: String, val number: String, val incoming: Boolean) {
        var connected = false
        var held = false
        var connection: AnvilConnection? = null

        /** The server's id for the call, as a push or its INVITE named it. */
        var serverCall: String? = null

        /** Answered at the screen before the phone's call came. */
        var answerWhenReady = false

        /** Shown from a push: the phone's call not here yet. */
        val pushed get() = id < 0
    }

    /** A pushed call rings this long without the phone's call, then stops. */
    private const val PUSHED_RING_MS = 60_000L

    /** Ids for pushed calls: below zero, apart from the phone's. */
    private var nextPushed = -1L

    /** Calls declined from a push before their INVITE came: when. */
    private val declined = HashMap<String, Long>()

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

    /** Dart hears what happened at the system, when it runs (its tests do). */
    private fun tell(action: String, id: Long, extra: Map<String, Any?> = emptyMap()) {
        val event = HashMap<String, Any?>(extra)
        event["action"] = action
        event["id"] = id
        main.post { channel?.invokeMethod("callEvent", event) }
    }

    /** What the user or the system did with a call: for the phone, and Dart. */
    fun emit(action: String, id: Long, extra: Map<String, Any?> = emptyMap()) {
        tell(action, id, extra)
        if (action == "shown") return
        if (id >= 0) {
            PhoneLink.act(action, id, extra)
            return
        }
        // A pushed call: the phone's is not here yet.
        main.post {
            val call = calls[id] ?: return@post
            when (action) {
                "answer" -> call.answerWhenReady = true
                "end" -> {
                    call.serverCall?.let { decline(it) }
                    CallService.appContext?.let { reportEnded(it, id) }
                }
            }
        }
    }

    private fun decline(serverCall: String) {
        val now = SystemClock.elapsedRealtime()
        declined.entries.removeAll { now - it.value > PUSHED_RING_MS }
        declined[serverCall] = now
    }

    /** What the phone says of its calls (`app/rust/src/system.rs`). */
    fun fromPhone(context: Context, event: JSONObject) {
        val id = event.optLong("id", -1)
        val name = event.optString("name")
        val number = event.optString("number")
        when (event.optString("kind")) {
            "incoming" -> {
                val serverCall = if (event.isNull("server_call")) null else event.optString("server_call")
                phoneRinging(context, id, name, number, serverCall)
            }
            "outgoing" -> if (!calls.containsKey(id)) reportOutgoing(context, id, name, number)
            "connected" -> reportConnected(context, id)
            "held" -> reportHeld(context, id, event.optBoolean("on"))
            "ended" -> reportEnded(context, id)
            // The phone stopped: its calls are over; a pushed call waits on.
            "reset" -> calls.values.filter { !it.pushed }.map { it.id }.forEach { reportEnded(context, it) }
        }
    }

    /** The phone's call ringing in: the pushed call it is, or a new one. */
    private fun phoneRinging(context: Context, id: Long, name: String, number: String, serverCall: String?) {
        if (calls.containsKey(id)) return
        if (serverCall != null && declined.remove(serverCall) != null) {
            PhoneLink.act("end", id)
            return
        }
        // FCP names its calls; a server that does not, the call a push
        // showed first.
        val pushed = calls.values.firstOrNull { it.pushed && serverCall != null && it.serverCall == serverCall }
            ?: calls.values.firstOrNull { it.pushed && serverCall == null }
        if (pushed == null) {
            reportIncoming(context, id, name, number)
            calls[id]?.serverCall = serverCall
            return
        }
        calls.remove(pushed.id)
        pushed.id = id
        calls[id] = pushed
        pushed.connection?.callId = id
        CallService.update(context)
        if (pushed.answerWhenReady) PhoneLink.act("answer", id)
    }

    /**
     * A push says a call is coming (`serverCall`, FCP's id for it): it rings
     * now, while the phone wakes and registers.
     */
    fun reportPushed(context: Context, serverCall: String, name: String, number: String) {
        if (calls.values.any { it.serverCall == serverCall } || declined.containsKey(serverCall)) return
        val id = nextPushed--
        val call = Call(id, name, number, incoming = true)
        call.serverCall = serverCall
        ring(context, call)
        main.postDelayed({
            if (calls[id]?.pushed == true) {
                Log.i(TAG, "the call $serverCall never came")
                reportEnded(context, id)
            }
        }, PUSHED_RING_MS)
    }

    /** The call a push told of is over (answered elsewhere, or given up). */
    fun cancelPushed(context: Context, serverCall: String) {
        val call = calls.values.firstOrNull { it.pushed && it.serverCall == serverCall } ?: return
        reportEnded(context, call.id)
    }

    /** The phone could not wake: the pushed calls cannot reach it. */
    fun wakeFailed(context: Context, serverCall: String) {
        val call = calls.values.firstOrNull { it.pushed && it.serverCall == serverCall } ?: return
        reportEnded(context, call.id)
    }

    private fun extras(call: Call) = Bundle().apply {
        putLong("id", call.id)
        putString("name", call.name)
        putString("number", call.number)
    }

    fun reportIncoming(context: Context, id: Long, name: String, number: String) {
        ring(context, Call(id, name, number, incoming = true))
    }

    private fun ring(context: Context, call: Call) {
        val id = call.id
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
        tell("ended", id)
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
