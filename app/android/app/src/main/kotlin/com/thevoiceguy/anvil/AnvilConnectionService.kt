package com.thevoiceguy.anvil

import android.net.Uri
import android.os.Build
import android.telecom.CallAudioState
import android.telecom.CallEndpoint
import android.telecom.Connection
import android.telecom.ConnectionRequest
import android.telecom.ConnectionService
import android.telecom.PhoneAccountHandle
import android.telecom.TelecomManager
import android.telecom.VideoProfile

/** Telecom's video state for a call with video, or without. */
fun videoState(video: Boolean) = if (video) VideoProfile.STATE_BIDIRECTIONAL else VideoProfile.STATE_AUDIO_ONLY

/**
 * One of the app's calls as Telecom holds it. A video call is one to
 * Telecom too: it routes the audio to the loudspeaker, as a video call's is.
 */
class AnvilConnection(callId: Long, name: String, number: String, video: Boolean) : Connection() {
    /** The phone's id for the call: a pushed call's changes when the phone's comes. */
    var callId: Long = callId

    init {
        connectionProperties = PROPERTY_SELF_MANAGED
        connectionCapabilities = CAPABILITY_HOLD or CAPABILITY_SUPPORT_HOLD or CAPABILITY_MUTE or
            CAPABILITY_SUPPORTS_VT_LOCAL_BIDIRECTIONAL or CAPABILITY_SUPPORTS_VT_REMOTE_BIDIRECTIONAL
        videoState = videoState(video)
        audioModeIsVoip = true
        setAddress(Uri.fromParts("sip", number, null), TelecomManager.PRESENTATION_ALLOWED)
        setCallerDisplayName(name, TelecomManager.PRESENTATION_ALLOWED)
    }

    override fun onAnswer() = CallSystem.emit("answer", callId)

    /** Answered at a headset or watch: audio only when it says so. */
    override fun onAnswer(videoState: Int) {
        if (VideoProfile.isAudioOnly(videoState)) {
            CallSystem.emit("answer", callId, mapOf("video" to false))
        } else {
            onAnswer()
        }
    }

    /** The call became a video call, or stopped being one. */
    fun showVideo(on: Boolean) = setVideoState(videoState(on))

    override fun onReject() = CallSystem.emit("end", callId)

    override fun onDisconnect() = CallSystem.emit("end", callId)

    override fun onAbort() = CallSystem.emit("end", callId)

    override fun onHold() = CallSystem.emit("hold", callId, mapOf("on" to true))

    override fun onUnhold() = CallSystem.emit("hold", callId, mapOf("on" to false))

    override fun onPlayDtmfTone(c: Char) = CallSystem.emit("dtmf", callId, mapOf("digits" to c.toString()))

    override fun onCallEndpointChanged(callEndpoint: CallEndpoint) =
        CallSystem.endpointsChanged(callEndpoint, null)

    override fun onAvailableCallEndpointsChanged(availableEndpoints: List<CallEndpoint>) =
        CallSystem.endpointsChanged(null, availableEndpoints)

    @Deprecated("Android 14 tells of call endpoints instead")
    override fun onCallAudioStateChanged(state: CallAudioState) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            @Suppress("DEPRECATION")
            CallSystem.audioStateChanged(state)
        }
    }

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
            extras.getBoolean("video") || VideoProfile.isVideo(request.videoState),
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
