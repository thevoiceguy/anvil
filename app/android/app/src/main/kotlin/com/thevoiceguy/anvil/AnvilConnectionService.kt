package com.thevoiceguy.anvil

import android.net.Uri
import android.telecom.Connection
import android.telecom.ConnectionRequest
import android.telecom.ConnectionService
import android.telecom.PhoneAccountHandle
import android.telecom.TelecomManager

/** One of the app's calls as Telecom holds it. */
class AnvilConnection(val callId: Long, name: String, number: String) : Connection() {
    init {
        connectionProperties = PROPERTY_SELF_MANAGED
        connectionCapabilities = CAPABILITY_HOLD or CAPABILITY_SUPPORT_HOLD or CAPABILITY_MUTE
        audioModeIsVoip = true
        setAddress(Uri.fromParts("sip", number, null), TelecomManager.PRESENTATION_ALLOWED)
        setCallerDisplayName(name, TelecomManager.PRESENTATION_ALLOWED)
    }

    override fun onAnswer() = CallSystem.emit("answer", callId)

    override fun onAnswer(videoState: Int) = onAnswer()

    override fun onReject() = CallSystem.emit("end", callId)

    override fun onDisconnect() = CallSystem.emit("end", callId)

    override fun onAbort() = CallSystem.emit("end", callId)

    override fun onHold() = CallSystem.emit("hold", callId, mapOf("on" to true))

    override fun onUnhold() = CallSystem.emit("hold", callId, mapOf("on" to false))

    override fun onPlayDtmfTone(c: Char) = CallSystem.emit("dtmf", callId, mapOf("digits" to c.toString()))

    /** A self-managed call shows its own incoming screen: the notification. */
    override fun onShowIncomingCallUi() {
        CallService.appContext?.let { CallService.update(it) }
    }
}

/** Telecom's way to the app's calls (self-managed: the app shows them). */
class AnvilConnectionService : ConnectionService() {
    private fun make(request: ConnectionRequest): AnvilConnection? {
        // Telecom hands the app's own extras over as the request's; some
        // versions leave them nested under the key they were sent with.
        val outer = request.extras ?: return null
        val extras = listOf(
            outer,
            outer.getBundle(TelecomManager.EXTRA_INCOMING_CALL_EXTRAS),
            outer.getBundle(TelecomManager.EXTRA_OUTGOING_CALL_EXTRAS),
        ).firstOrNull { it?.containsKey("id") == true } ?: return null
        return AnvilConnection(
            extras.getLong("id"),
            extras.getString("name") ?: "",
            extras.getString("number") ?: "",
        )
    }

    override fun onCreateIncomingConnection(
        account: PhoneAccountHandle?,
        request: ConnectionRequest,
    ): Connection {
        CallService.appContext = applicationContext
        val connection = make(request)
            ?: return Connection.createFailedConnection(
                android.telecom.DisconnectCause(android.telecom.DisconnectCause.ERROR),
            )
        connection.setRinging()
        CallSystem.attached(applicationContext, connection)
        return connection
    }

    override fun onCreateOutgoingConnection(
        account: PhoneAccountHandle?,
        request: ConnectionRequest,
    ): Connection {
        CallService.appContext = applicationContext
        val connection = make(request)
            ?: return Connection.createFailedConnection(
                android.telecom.DisconnectCause(android.telecom.DisconnectCause.ERROR),
            )
        connection.setDialing()
        CallSystem.attached(applicationContext, connection)
        return connection
    }

    override fun onCreateIncomingConnectionFailed(account: PhoneAccountHandle?, request: ConnectionRequest) {
        make(request)?.let { CallSystem.refused(applicationContext, it.callId) }
    }

    override fun onCreateOutgoingConnectionFailed(account: PhoneAccountHandle?, request: ConnectionRequest) {
        make(request)?.let { CallSystem.refused(applicationContext, it.callId) }
    }
}
